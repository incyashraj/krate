/* The organisation dashboard's Addendum 3 parts (IC-947, IC-948): the
 * closing, closed, suspended and recovery banners; closing and export;
 * replacing the root key; company sign-in; offline access; billing.
 *
 * dashboard.js owns the page and hands these parts what they need (D).
 * Each part asks the hub when it is drawn and shows refusals in words,
 * exactly like the rest of the page.
 */
(function (root) {
  "use strict";

  root.KrateOrgParts = function (D) {
    var K = root.KrateOrg;
    var el = K.el;
    var slug = D.slug;
    var path = function (p) { return "/orgs/" + slug + (p || ""); };
    var nameOf = function () { return (D.S.org && D.S.org.name) || slug; };
    var owner = function () { return D.roleOf() === "owner"; };
    var now = function () { return Math.floor(Date.now() / 1000); };
    var ownersCount = function () {
      return (D.S.members || []).filter(function (m) { return m.role === "owner" && (m.state || "active") === "active"; }).length;
    };

    /// One line of a refusal, for a small note under a control.
    function line(res, ctx) {
      var w = D.words(res, ctx);
      return w.title + ". " + w.body;
    }

    // ------------------------------------------------------------ banners

    /// What the organisation's state means right now, above the tabs.
    function banner(box) {
      box.textContent = "";
      var o = D.S.org || {};
      var name = nameOf();
      var add = function (cls, title, body, actions) {
        actions = (actions || []).filter(Boolean);
        box.appendChild(el("div", { class: "call " + cls + " org-why banner", role: "status" }, [
          el("div", {}, [el("b", { text: title }), el("p", { text: body }), actions && actions.length ? el("div", { class: "bacts" }, actions) : null]),
        ]));
      };
      var toSettings = D.S.tab === "settings" ? null : el("button", { class: "btn ghost sm", type: "button", text: "Open Settings", onclick: function () { D.show("settings"); } });
      if (o.state === "closed") {
        add("bad", name + " closed" + (o.closed_at ? " on " + K.day(o.closed_at) : ""),
          "Its apps no longer open. Its export stays downloadable to former owners for a while: build or download it in Settings.", [toSettings]);
      } else if (o.close) {
        var c = o.close;
        if (c.state === "waiting-for-second-owner") {
          add("warn", "Closing " + name + " waits for a second owner",
            K.who(c.requested_by).label + " asked to close it on " + K.day(c.requested_at) + ". Another owner approves the closing in Settings, or any owner cancels it there. Until then nothing changes.",
            owner() ? [toSettings] : null);
        } else {
          add("warn", name + " closes on " + K.day(c.ends_at),
            "Its apps still open until then, but nothing new can be sent or approved. Owners can cancel the closing or download an export in Settings.",
            owner() ? [toSettings] : null);
        }
      }
      if (o.state !== "closed" && (o.suspended || o.billing_state === "suspended")) {
        var acts = K.can(D.roleOf(), "billing.read") ? [el("button", { class: "btn ghost sm", type: "button", text: "Open Billing", onclick: function () { D.show("billing"); } })] : null;
        add("warn", name + " is paused for billing",
          "Apps already released still open, and the export still works. Sending new apps, seats beyond the free ones and company sign-in come back when billing is settled.", acts);
      }
      var r = o.recovery;
      if (r && !/^(cancelled|completed|done|applied|objected)$/.test(String(r.state || "")) && (D.roleOf() === "owner" || D.roleOf() === "admin")) {
        var object = el("button", { class: "btn dark sm", type: "button", text: "Object" });
        object.addEventListener("click", function () {
          var key = K.key();
          D.confirmSheet({
            title: "Object to this recovery?",
            body: "Krate support will not make " + K.who(r.for).label + " an owner, and the request is cancelled. The activity records your objection.",
            yes: "Object and cancel it",
            no: "Go back",
            busy: "Objecting",
            ctx: { what: "object to a recovery", whoCan: "owners and admins" },
            act: function () { return K.api("POST", path("/recover/" + encodeURIComponent(r.id) + "/object"), {}, { key: key }); },
            done: function () { D.toast("Objection recorded. Nobody gains ownership."); D.refresh(); },
          });
        });
        add("warn", "Krate support is restoring ownership of " + name,
          K.who(r.by).label + " asked on " + K.day(r.requested_at) + " to make " + K.who(r.for).label + " an owner, because: “" + (r.reason || "no reason given") + "”. It takes effect on " + K.moment(r.effective_at) + " unless an owner or admin objects." + (Array.isArray(r.not_mailed) && r.not_mailed.length ? " Not mailed: " + r.not_mailed.map(function (p) { return K.who(p).label; }).join(", ") + "." : ""),
          [object]);
      }
      box.hidden = !box.children.length;
    }

    /// Tabs while closed: former owners keep only Settings, for the export.
    function tabs(list) {
      var o = D.S.org || {};
      if (o.state === "closed") return ["settings"];
      return list;
    }

    // --------------------------------------------------------- invitations

    /// Was the invitation mailed (Addendum 3 A)? Words above the link.
    function inviteMail(ans, sent) {
      if (ans.emailed === true) {
        return { title: "Emailed to " + sent.email, body: "It works once, only for that inbox, until " + K.day(ans.expires_at) + ". The link is below too, if you want to send it yourself." };
      }
      return {
        title: "Copy this link and send it",
        // The hub's sentence, without a trailing "-- copy the link" the
        // title already says.
        body: (ans.email_problem ? String(ans.email_problem).split(" -- ")[0].replace(/\.?$/, ". ") : "") + "It works once, only for " + (sent.github ? "@" + sent.github + " signed in with GitHub" : sent.email + " signed in with that inbox") + ", until " + K.day(ans.expires_at) + ".",
      };
    }

    // ------------------------------------------------------------ root key

    /// "Replace root key" (Addendum 3 C): a 64-hex public key, pasted or
    /// read from a .pub file, never a private key.
    function replaceKey(card) {
      var open = el("button", { class: "btn ghost sm", type: "button", text: "Replace root key" });
      var form = el("div", { class: "keyform", hidden: true });
      var inp = el("input", { class: "inp", id: "newKey", autocomplete: "off", spellcheck: "false", "aria-label": "New public key", placeholder: "The new public key: 64 characters, 0-9 and a-f" });
      var file = el("input", { type: "file", id: "newKeyFile", accept: ".pub,text/plain", class: "vh", "aria-label": "Choose a .pub file" });
      var pick = el("label", { class: "btn ghost sm", for: "newKeyFile", text: "Choose a .pub file" });
      var note = el("p", { class: "note", hidden: true });
      var go = el("button", { class: "btn dark sm", type: "button", text: "Replace" });
      var say = function (t) { note.textContent = t; note.hidden = !t; };
      file.addEventListener("change", function () {
        var f = file.files && file.files[0];
        if (!f) return;
        if (f.size > 4096) { say("That file is too big to be a public key. Choose the .pub file, never the private key."); return; }
        f.text().then(function (t) { inp.value = t.trim(); check(); });
      });
      function check() {
        var v = inp.value.trim().toLowerCase();
        if (/^[0-9a-f]{64}$/.test(v)) { say(""); return v; }
        if (/PRIVATE KEY|pkcs8/i.test(inp.value) || inp.value.length > 200) {
          say("That looks like a private key. Keep it to yourself: Krate only needs the public key, the .pub file.");
        } else if (inp.value.trim()) {
          say("A public key is 64 characters of 0-9 and a-f, the contents of the .pub file.");
        }
        return "";
      }
      inp.addEventListener("input", function () { if (note.hidden === false) check(); });
      open.addEventListener("click", function () { form.hidden = !form.hidden; open.hidden = !form.hidden; });
      go.addEventListener("click", function () {
        var v = check();
        if (!v) { if (!inp.value.trim()) say("Paste the new public key, or choose its .pub file."); return; }
        var key = K.key();
        D.confirmSheet({
          title: "Replace " + nameOf() + "'s root key?",
          body: "From now on only apps signed by the new key, or by release keys it delegates, can be sent here. Apps already released keep opening. Activity records the change.",
          digest: K.fingerprint(v),
          digestLabel: "New key fingerprint",
          yes: "Replace root key",
          busy: "Replacing",
          ctx: { what: "replace the root key", whoCan: "an owner" },
          act: function () { return K.api("PUT", path("/root-key"), { public_key: v }, { key: key }); },
          done: function () { D.toast("Root key replaced. Activity says so."); D.show("settings"); },
        });
      });
      form.appendChild(el("p", { class: "sub", text: "Paste the new root's public key, or choose its .pub file. Only the public half ever leaves your computer." }));
      form.appendChild(inp);
      form.appendChild(el("div", { class: "row2" }, [pick, file, go]));
      form.appendChild(note);
      card.appendChild(el("div", { class: "row2" }, [open]));
      card.appendChild(form);
    }

    // --------------------------------------------------------- offline

    function offlineSelect(value, label, onPick) {
      var sel = el("select", { class: "sel", "aria-label": label });
      K.OFFLINE.forEach(function (o) {
        var opt = el("option", { value: o[0], text: o[1] });
        if (o[0] === value) opt.selected = true;
        sel.appendChild(opt);
      });
      sel.addEventListener("change", function () { onPick(sel); });
      return sel;
    }

    /// The release row's offline class: words for everyone, a choice for
    /// owners (Addendum 3 G1).
    function releaseOffline(r, note) {
      var cls = r.offline || (D.S.org && D.S.org.offline_default) || "days-7";
      if (!K.can(D.roleOf(), "offline.set") || (D.S.org && D.S.org.state === "closing")) {
        return el("span", { class: "tag", text: K.offlineName(cls) });
      }
      return offlineSelect(cls, "Offline access for " + (r.name || "this app"), function (sel) {
        var was = cls;
        sel.disabled = true;
        K.api("PUT", path("/apps/" + encodeURIComponent(r.name) + "/offline"), { class: sel.value }, { key: K.key() }).then(function (res) {
          sel.disabled = false;
          if (res.ok) { cls = sel.value; D.toast(K.appName(r) + ": " + K.offlineName(cls).toLowerCase() + ". New opens get the new terms."); return; }
          sel.value = was;
          note.textContent = line(res, { what: "change offline access", whoCan: "an owner" });
          note.hidden = false;
        });
      });
    }

    function offlineCard() {
      var o = D.S.org || {};
      var cls = o.offline_default || "days-7";
      var card = el("div", { class: "card" }, [
        el("h3", { text: "Offline access" }),
        el("p", { text: "How long a released app opens without a connection, before it has to check that the person is still in " + nameOf() + ". Each app can differ, in Releases. Nothing longer than 30 days exists." }),
      ]);
      if (!owner()) {
        card.appendChild(el("p", { class: "fpline", text: "New apps: " + K.offlineName(cls) }));
        return card;
      }
      var note = el("p", { class: "note", hidden: true });
      var sel = offlineSelect(cls, "Offline access for new apps", function () {
        sel.disabled = true;
        K.api("PUT", path("/offline-default"), { class: sel.value }, { key: K.key() }).then(function (res) {
          sel.disabled = false;
          if (res.ok) { cls = sel.value; D.toast("New apps in " + nameOf() + ": " + K.offlineName(cls).toLowerCase() + "."); return; }
          sel.value = cls;
          note.textContent = line(res, { what: "change offline access", whoCan: "an owner" });
          note.hidden = false;
        });
      });
      card.appendChild(el("div", { class: "row2" }, [el("span", { class: "lbl", text: "New apps" }), sel]));
      card.appendChild(note);
      return card;
    }

    // -------------------------------------------------- company sign-in

    function ssoCard() {
      var card = el("div", { class: "card", id: "ssoCard" }, [
        el("h3", { text: "Company sign-in" }),
        el("p", { text: "Let people sign in with your company's accounts (OpenID Connect). Paste what your identity provider gives you when you add Krate as an app there." }),
        el("p", { class: "loading", text: "Loading" }),
      ]);
      // Whether SAML signs people in (doors.js): then this card holds only
      // the domain, joining and enforcement, and needs no OIDC issuer.
      Promise.all([K.api("GET", path("/sso")), K.api("GET", path("/saml"))]).then(function (both) {
        var res = both[0];
        card.lastChild.remove();
        if (!res.ok) { card.appendChild(K.refusalBlock(D.words(res, { what: "set up company sign-in", whoCan: "an owner" }))); return; }
        samlOn = Boolean(both[1].ok && both[1].body && both[1].body.idp);
        ssoForm(card, res.body || null);
      });
      return card;
    }

    var samlOn = false;
    function ssoForm(card, cur) {
      cur = cur || {};
      var oidcOff = samlOn && !cur.issuer;
      var fld = function (label, input, hint) { return el("label", { class: "fld" }, [el("span", { text: label }), input, hint ? el("small", { text: hint }) : null]); };
      var issuer = el("input", { class: "inp", id: "ssoIssuer", value: cur.issuer || "", autocomplete: "off", spellcheck: "false", placeholder: "https://accounts.google.com" });
      var client = el("input", { class: "inp", id: "ssoClient", value: cur.client_id || "", autocomplete: "off", spellcheck: "false", placeholder: "The client ID your provider shows" });
      var secret = el("input", { class: "inp", id: "ssoSecret", type: "password", autocomplete: "new-password", placeholder: cur.has_secret ? "Stored. Leave empty to keep it" : "The client secret" });
      var domain = el("input", { class: "inp", id: "ssoDomain", value: cur.domain || "", autocomplete: "off", spellcheck: "false", placeholder: "acme.com" });
      var join = el("select", { class: "sel", id: "ssoJoin", "aria-label": "Who joins by themselves" });
      [["", "Nobody joins by themselves"], ["developer", "Join as developer"], ["reviewer", "Join as reviewer"], ["recipient", "Join as recipient"]].forEach(function (o) {
        var opt = el("option", { value: o[0], text: o[1] });
        if ((cur.auto_join_role || "") === o[0]) opt.selected = true;
        join.appendChild(opt);
      });
      var enforce = el("input", { type: "checkbox", id: "ssoEnforce" });
      enforce.checked = !!cur.enforce;
      if (!cur.domain_verified) enforce.disabled = true;
      var chips = el("div", { class: "chips" }, K.PROVIDERS.map(function (p) {
        return el("button", { class: "chip", type: "button", text: p[0], onclick: function () {
          issuer.value = /</.test(p[1]) ? "" : p[1];
          issuer.placeholder = p[1];
          issuer.focus();
          shape.textContent = p[0] + ": " + p[1] + (p[2] ? ", " + p[2] : "");
        } });
      }));
      var shape = el("small", { text: K.PROVIDERS.map(function (p) { return p[0] + ": " + p[1] + (p[2] ? ", " + p[2] : ""); }).join(" · ") });
      var out = el("div");
      var save = el("button", { class: "btn dark", type: "button", text: cur.issuer || oidcOff ? "Save changes" : "Turn on company sign-in" });
      save.addEventListener("click", function () {
        var body = {
          issuer: issuer.value.trim(),
          client_id: client.value.trim(),
          domain: domain.value.trim().toLowerCase().replace(/^@/, ""),
          auto_join_role: join.value || null,
          enforce: !!enforce.checked,
        };
        if (secret.value) body.client_secret = secret.value;
        out.textContent = "";
        // https, or a loopback issuer on a copy of the site served from
        // this machine (the hub allows that only on a development hub too).
        var local = /^(127\.0\.0\.1|localhost)$/.test(location.hostname) && /^http:\/\/(127\.0\.0\.1|localhost)(:\d+)?\//.test(body.issuer + "/");
        if (oidcOff && !body.issuer) { delete body.client_id; delete body.issuer; }
        else if (!/^https:\/\/[^\s<>]+$/.test(body.issuer) && !local) return hintOut("The issuer is a web address", "It starts with https://, like " + K.PROVIDERS[0][1] + ". Pick your provider above to see its shape.");
        if (!oidcOff && !body.client_id) return hintOut("The client ID is missing", "Your provider shows it when you add Krate as an app.");
        if (!oidcOff && !cur.has_secret && !body.client_secret) return hintOut("The client secret is missing", "Your provider shows it once, when you create the app. It is stored encrypted and never shown again.");
        if (!/^[a-z0-9.-]+\.[a-z]{2,}$/.test(body.domain)) return hintOut("The domain is a name like acme.com", "The part of your people's work addresses after the @.");
        save.disabled = true;
        save.textContent = "Saving";
        K.api("PUT", path("/sso"), body, { key: K.key() }).then(function (res) {
          save.disabled = false;
          save.textContent = "Save changes";
          if (!res.ok) { out.appendChild(K.refusalBlock(D.words(res, { what: "set up company sign-in", whoCan: "an owner" }), "bad")); return; }
          secret.value = "";
          D.toast("Company sign-in saved." + (res.body && res.body.domain_verified ? "" : " Prove the domain below to let it auto-join and enforce."));
          var fresh = card;
          fresh.textContent = "";
          fresh.appendChild(el("h3", { text: "Company sign-in" }));
          fresh.appendChild(el("p", { text: "Let people sign in with your company's accounts (OpenID Connect). Paste what your identity provider gives you when you add Krate as an app there." }));
          ssoForm(fresh, res.body && res.body.issuer ? res.body : body);
        });
      });
      function hintOut(t, b) { out.appendChild(K.refusalBlock({ title: t, body: b, action: null }, "warn")); }

      card.appendChild(el("div", { class: "fld" }, [el("span", { text: "Your provider" }), chips, shape]));
      card.appendChild(el("div", { class: "form sso" }, [
        fld("Issuer", issuer),
        fld("Client ID", client),
        fld("Client secret", secret, "Stored encrypted. Krate never shows it again, and never writes it in Activity."),
        fld("Company domain", domain, "Only verified addresses at this domain sign in as your people."),
        fld("Joining", join, "Someone at your domain who signs in can join by themselves, never as owner or admin."),
        el("label", { class: "chk" }, [enforce, el("span", { text: cur.domain_verified
          ? "Require company sign-in: people at " + cur.domain + " must use it to reach " + nameOf() + "."
          : "Require company sign-in. Available once the domain is verified." })]),
      ]));
      card.appendChild(out);
      card.appendChild(el("div", { class: "row2" }, [save]));
      // From Getting started (IC-958 B2): the provider the owner picked and
      // their company's domain, already in; the person only pastes what
      // their provider shows.
      var pre = D.S.ssoPrefill;
      if (pre && !cur.issuer) {
        D.S.ssoPrefill = null;
        if (!domain.value && pre.domain) domain.value = pre.domain;
        var at = { google: 0, okta: 1, microsoft: 2 }[pre.provider];
        if (at !== undefined && chips.children[at]) chips.children[at].click();
        var target = pre.provider === "saml" ? root.document.getElementById("samlCard") || card : card;
        if (target.scrollIntoView) { try { target.scrollIntoView({ behavior: "smooth", block: "start" }); } catch (e) {} }
        if (pre.provider !== "saml") { try { (at === 0 ? client : issuer).focus(); } catch (e) {} }
      }
      if (cur.domain) card.appendChild(domainProof(cur));
      if (cur.issuer) {
        // Turning it off (DELETE /orgs/<slug>/sso), with a confirmation.
        var off = el("button", { class: "btn quiet sm", type: "button", text: "Turn off company sign-in" });
        off.addEventListener("click", function () {
          var key = K.key();
          D.confirmSheet({
            title: "Turn off company sign-in for " + nameOf() + "?",
            body: "Nobody can sign in through it any more, nothing is required, and the stored client secret and domain proof are deleted. Members stay members and sign in with GitHub, Google or email.",
            yes: "Turn it off",
            no: "Keep it on",
            busy: "Turning off",
            ctx: { what: "turn off company sign-in", whoCan: "an owner" },
            act: function () { return K.api("DELETE", path("/sso"), null, { key: key }); },
            done: function () { D.toast("Company sign-in is off for " + nameOf() + "."); D.show("settings"); },
          });
        });
        var offRow = el("div", { class: "row2" }, [off]);
      }
      if (cur.issuer) {
        var link = location.origin + "/login/?org=" + slug;
        var cp = el("button", { class: "btn ghost sm", type: "button", text: "Copy", onclick: function () { K.copy(link, cp); } });
        card.appendChild(el("div", { class: "proof" }, [
          el("b", { text: "Your people sign in here" }),
          el("div", { class: "linkbox" }, [el("code", { class: "k", text: link }), cp]),
        ]));
        card.appendChild(offRow);
      }
    }

    function domainProof(cur) {
      var box = el("div", { class: "proof", id: "ssoProof" });
      if (cur.domain_verified) {
        box.appendChild(el("p", {}, [el("span", { class: "tag ok", text: "Verified" }), " " + cur.domain + (cur.verified_at ? " since " + K.day(cur.verified_at) : "")]));
        return box;
      }
      box.appendChild(el("b", { text: "Prove you own " + cur.domain }));
      box.appendChild(el("p", { class: "sub", text: "Add this TXT record at your DNS provider, wait a few minutes, then press Verify. Until then company sign-in works, but nobody joins by themselves and nothing is required." }));
      var rec = el("div", { class: "recs" }, [el("p", { class: "sub", text: "Loading" })]);
      box.appendChild(rec);
      var out = el("div");
      var verify = el("button", { class: "btn dark sm", type: "button", text: "Verify" });
      verify.addEventListener("click", function () {
        verify.disabled = true;
        verify.textContent = "Checking";
        K.api("POST", path("/sso/verify-domain"), {}, { key: K.key() }).then(function (res) {
          verify.disabled = false;
          verify.textContent = "Verify";
          out.textContent = "";
          if (res.ok && res.body && res.body.domain_verified) { D.toast(cur.domain + " is verified."); D.show("settings"); return; }
          out.appendChild(K.refusalBlock(D.words(res.ok ? { body: { error: "domain-not-verified" } } : res, { what: "verify the domain", whoCan: "an owner" }), "warn"));
        });
      });
      K.api("GET", path("/sso/domain-token")).then(function (res) {
        rec.textContent = "";
        if (!res.ok) { rec.appendChild(el("p", { class: "note", text: line(res, {}) })); return; }
        var copyBtn = function (v) { var b = el("button", { class: "btn ghost sm", type: "button", text: "Copy" }); b.addEventListener("click", function () { K.copy(v, b); }); return b; };
        // One line per part of the record: what it is, the text, Copy.
        rec.appendChild(el("div", { class: "rec" }, [el("span", { text: "Type" }), el("code", { class: "k", text: "TXT" }), null]));
        rec.appendChild(el("div", { class: "rec" }, [el("span", { text: "Name" }), el("code", { class: "k", text: res.body.name }), copyBtn(res.body.name)]));
        rec.appendChild(el("div", { class: "rec" }, [el("span", { text: "Value" }), el("code", { class: "k", text: res.body.value }), copyBtn(res.body.value)]));
      });
      box.appendChild(el("div", { class: "row2" }, [verify]));
      box.appendChild(out);
      return box;
    }

    // ------------------------------------------------------ close, export

    function exportCard() {
      var card = el("div", { class: "card", id: "exportCard" }, [
        el("h3", { text: "Export" }),
        el("p", { text: "One zip of everything " + nameOf() + " holds: its releases' files, what was sent for approval, members and roles, and the activity with its chain. Never keys, tokens or secrets." }),
      ]);
      var list = el("div", { class: "rows exports" });
      var out = el("div");
      var build = el("button", { class: "btn dark sm", type: "button", text: "Build an export" });
      build.addEventListener("click", function () {
        build.disabled = true;
        K.api("POST", path("/export"), {}, { key: K.key() }).then(function (res) {
          build.disabled = false;
          out.textContent = "";
          if (!res.ok) { out.appendChild(K.refusalBlock(D.words(res, { what: "export", whoCan: "an owner" }), "bad")); return; }
          load();
        });
      });
      var timer = null;
      function load() {
        clearTimeout(timer);
        K.api("GET", path("/exports")).then(function (res) {
          list.textContent = "";
          if (!res.ok) { list.appendChild(el("p", { class: "note", text: line(res, { what: "see exports", whoCan: "an owner" }) })); return; }
          var items = Array.isArray(res.body) ? res.body : (res.body && res.body.exports) || [];
          if (!items.length) { list.appendChild(el("p", { class: "sub", text: "No export yet." })); return; }
          var building = false;
          items.slice().sort(function (a, b) { return (b.created_at || 0) - (a.created_at || 0); }).forEach(function (x) {
            building = building || x.state === "building";
            var note = el("p", { class: "note", hidden: true });
            var acts = [];
            if (x.state === "ready") {
              var dl = el("button", { class: "btn ghost sm", type: "button", text: "Download" });
              dl.addEventListener("click", function () {
                dl.disabled = true;
                var file = slug + "-export-" + new Date((x.created_at || now()) * 1000).toISOString().slice(0, 10) + ".zip";
                K.download(K.hub() + path("/exports/" + encodeURIComponent(x.id)), file).then(function (r) {
                  dl.disabled = false;
                  note.textContent = r.ok ? "Saved as " + file + "." : line({ body: r.body }, {});
                  note.hidden = false;
                });
              });
              acts.push(dl);
            }
            var tag = { building: ["Building", "warn"], ready: ["Ready", "ok"], failed: ["Failed", ""] }[x.state] || [x.state, ""];
            list.appendChild(el("div", { class: "rw flat" }, [
              el("div", { class: "t" }, [
                el("b", {}, [el("span", { class: "tag " + tag[1], text: tag[0] }), " Built " + K.moment(x.created_at)]),
                el("small", { text: (typeof x.size === "number" ? Math.max(1, Math.round(x.size / 1024)) + " KB · " : "") + (x.expires_at ? "kept until " + K.day(x.expires_at) : "") + (x.state === "failed" ? "Build another one." : "") }),
                note,
              ]),
              acts.length ? el("div", { class: "ra" }, acts) : null,
            ]));
          });
          // Building takes a moment; look again while this tab is open.
          if (building) timer = setTimeout(function () { if (document.getElementById("exportCard") === card || card.parentNode) load(); }, 3000);
        });
      }
      card.appendChild(el("div", { class: "row2" }, [build]));
      card.appendChild(out);
      card.appendChild(list);
      load();
      return card;
    }

    /// The closing statement in words: the hub's `effect` sentence, or
    /// the statement itself when it is already text.
    function statementText(st) {
      if (!st) return "";
      if (typeof st === "string") return st;
      return st.effect || "";
    }

    function closeCard() {
      var o = D.S.org || {};
      var name = nameOf();
      var card = el("div", { class: "card danger", id: "closeCard" }, [el("h3", { text: "Close " + name })]);
      var out = el("div");
      // A close in progress: waiting for a second owner (the organisation
      // is still active), or scheduled (closing).
      if (o.close) {
        var c = o.close;
        var waiting = c.state === "waiting-for-second-owner";
        card.appendChild(el("p", { text: waiting
          ? K.who(c.requested_by).label + " asked to close " + name + " on " + K.day(c.requested_at) + ". It needs a second owner's approval before the closing starts."
          : name + " closes on " + K.moment(c.ends_at) + (c.approved_by ? ", approved by " + K.who(c.approved_by).label : "") + ". Until then its apps open, and nothing new can be sent or approved." }));
        var said = statementText(c.statement);
        if (said) card.appendChild(el("div", { class: "stmt" }, [el("span", { text: "What closing does" }), el("p", { text: said })]));
        var acts = [];
        if (waiting && !D.isMe(c.requested_by)) {
          var ap = el("button", { class: "btn dark sm", type: "button", text: "Approve the closing" });
          ap.addEventListener("click", function () {
            var key = K.key();
            D.confirmSheet({
              title: "Approve closing " + name + "?",
              body: "The closing starts now and ends on the date it names. Its apps open until then. Any owner can still cancel it before it ends.",
              digest: c.statement_digest,
              digestLabel: "The statement you approve (sha256)",
              yes: "Approve the closing",
              busy: "Approving",
              ctx: { what: "approve the closing", whoCan: "a second owner" },
              act: function () { return K.api("POST", path("/close/" + encodeURIComponent(c.id) + "/approve"), { statement_digest: c.statement_digest }, { key: key }); },
              done: function () { D.toast("Approved. " + name + " is closing."); D.refresh(); },
            });
          });
          acts.push(ap);
        } else if (waiting) {
          card.appendChild(el("p", { class: "sub", text: "You asked for it, so another owner approves it." }));
        }
        var cancel = el("button", { class: "btn ghost sm", type: "button", text: "Cancel the closing" });
        cancel.addEventListener("click", function () {
          var key = K.key();
          D.confirmSheet({
            title: "Cancel closing " + name + "?",
            body: "Nothing closes. Everything goes on as before, and Activity records the cancellation.",
            yes: "Cancel the closing",
            no: "Keep it closing",
            busy: "Cancelling",
            ctx: { what: "cancel the closing", whoCan: "an owner" },
            act: function () { return K.api("DELETE", path("/close/" + encodeURIComponent(c.id)), null, { key: key }); },
            done: function () { D.toast("The closing is cancelled. " + name + " goes on as before."); D.refresh(); },
          });
        });
        acts.push(cancel);
        card.appendChild(el("div", { class: "row2" }, acts));
        card.appendChild(out);
        return card;
      }
      var sole = ownersCount() <= 1;
      card.appendChild(el("p", { text: "Closing starts a window before anything stops: apps keep opening, nothing new is sent, and owners can cancel it or take an export. When it ends, the apps stop opening and only the export stays, for a while." + (sole ? "" : " With more than one owner, a second owner approves it first.") }));
      var go = el("button", { class: "btn quiet sm", type: "button", text: "Close " + name });
      go.addEventListener("click", function () {
        var key = K.key();
        D.confirmSheet({
          title: "Close " + name + "?",
          body: sole
            ? "Type " + slug + " to confirm. The window starts now; you can cancel it until it ends."
            : "A second owner has to approve it. Nothing changes until they do, and any owner can cancel it.",
          input: sole ? { label: "Type " + slug + " to confirm", placeholder: slug } : null,
          yes: sole ? "Close " + name : "Ask a second owner",
          no: "Keep it open",
          busy: "Asking",
          ctx: { what: "close the organisation", whoCan: "an owner" },
          act: function (typed) { return K.api("POST", path("/close"), sole ? { confirm_slug: String(typed || "").trim() } : {}, { key: key }); },
          done: function () { D.toast(sole ? name + " is closing. Its apps open until the window ends." : "Asked. Another owner approves it in Settings."); D.refresh(); },
        });
      });
      card.appendChild(el("div", { class: "row2" }, [go]));
      card.appendChild(out);
      return card;
    }

    /// Settings additions, in order, below the root key and publish cards.
    function settings(pane, keyCard) {
      var o = D.S.org || {};
      if (o.state === "closed") {
        pane.appendChild(exportCard());
        return;
      }
      if (owner() && o.root_key) replaceKey(keyCard);
      pane.appendChild(offlineCard());
      if (K.can(D.roleOf(), "sso.set")) pane.appendChild(ssoCard());
      // IC-955: SAML, provisioning, and the activity sent to the company's logs.
      var doors = root.KrateOrgDoors ? root.KrateOrgDoors(D) : null;
      if (doors && K.can(D.roleOf(), "saml.set")) pane.appendChild(doors.samlCard());
      if (doors && K.can(D.roleOf(), "scim.manage")) pane.appendChild(doors.scimCard());
      if (doors && K.can(D.roleOf(), "audit-export.view")) pane.appendChild(doors.exportCard());
      // G12: notices in the company's Slack and Teams channels.
      if (doors && K.can(D.roleOf(), "chat.manage") === "yes") pane.appendChild(doors.chatCard());
      if (K.can(D.roleOf(), "org.export")) pane.appendChild(exportCard());
      if (K.can(D.roleOf(), "org.close")) pane.appendChild(closeCard());
    }

    // ------------------------------------------------------------ billing

    var PLAN = { pilot: "Pilot", seats: "Per seat", paid: "Paid by card", stripe: "Paid by card", invoiced: "Invoiced by Krate", manual: "Invoiced by Krate" };
    var STATE = {
      pilot: ["Pilot: everything is on", "ok"], paid: ["Paid", "ok"], active: ["Paid", "ok"], invoiced: ["Invoiced", "ok"],
      "past-due": ["Payment overdue", "warn"], suspended: ["Paused for billing", "warn"], canceled: ["Cancelled", ""],
    };

    function billing(pane) {
      K.api("GET", path("/billing")).then(function (res) {
        D.head(pane, "Billing", "Seats are owners, admins, developers and reviewers. Recipients are free.");
        if (!res.ok) return D.failed(pane, res, { what: "see billing", whoCan: "owners and admins" });
        var b = res.body || {};
        var st = STATE[b.state] || [b.state || "Unknown", ""];
        var seats = (b.seats_used != null ? b.seats_used + " used" : "") + (b.seats_paid != null ? " · " + b.seats_paid + " paid" : "");
        var dl = [
          el("dt", { text: "Plan" }), el("dd", { text: PLAN[b.plan] || b.plan || "Pilot" }),
          el("dt", { text: "Seats" }), el("dd", { text: seats || "None yet" }),
        ];
        if (b.period_end && !b.manual && b.state !== "pilot") { dl.push(el("dt", { text: "Paid until" })); dl.push(el("dd", { text: K.day(b.period_end) })); }
        if (b.manual) {
          dl.push(el("dt", { text: "Invoice" }));
          dl.push(el("dd", { text: b.manual.reference + (b.manual.paid_until ? ", paid until " + K.day(b.manual.paid_until) : "") }));
        }
        var plan = el("div", { class: "card" }, [
          el("div", { class: "row2" }, [el("h3", { text: nameOf() + "'s plan" }), el("span", { class: "tag " + st[1], text: st[0] })]),
          el("dl", { class: "kv" }, dl),
        ]);
        if (b.state === "pilot") plan.appendChild(el("p", { class: "sub", text: "While it is a pilot, everything is on and nothing is charged." }));
        if (b.checkout_open && owner()) {
          var out = el("div");
          var pay = el("button", { class: "btn dark", type: "button", text: "Pay by card" + (b.seats_used ? " for " + b.seats_used + (b.seats_used === 1 ? " seat" : " seats") : "") });
          var key = K.key();
          pay.addEventListener("click", function () {
            pay.disabled = true;
            pay.textContent = "Opening checkout";
            K.api("POST", path("/billing/checkout"), {}, { key: key }).then(function (r) {
              pay.disabled = false;
              pay.textContent = "Pay by card";
              out.textContent = "";
              if (r.ok && r.body && /^https:\/\//.test(r.body.url || "")) { location.href = r.body.url; return; }
              out.appendChild(K.refusalBlock(D.words(r.ok ? { body: { error: "hub-error" } } : r, { what: "pay", whoCan: "an owner" }), "bad"));
            });
          });
          plan.appendChild(el("div", { class: "row2" }, [pay]));
          plan.appendChild(el("p", { class: "sub", text: "Checkout is Stripe's page; the price is shown there before you pay." }));
          plan.appendChild(out);
        } else if (!b.checkout_open) {
          plan.appendChild(el("p", { class: "invline" }, ["Krate invoices pilot organisations directly: write to ", el("a", { href: "mailto:yashraj@krate.tech", text: "yashraj@krate.tech" }), "."]));
        }
        pane.appendChild(plan);
        var inv = el("div", { class: "card rows" }, [el("p", { class: "grp", text: "Invoices" })]);
        var list = Array.isArray(b.invoices) ? b.invoices : [];
        if (!list.length) inv.appendChild(el("p", { class: "sub", style: "padding:6px 0 14px", text: "No invoices yet." }));
        list.forEach(function (i) {
          var amount = K.money(i.amount, i.currency);
          inv.appendChild(el("div", { class: "rw flat" }, [
            el("div", { class: "t" }, [
              el("b", { text: (i.number || (b.manual && i.id === b.manual.reference ? "Invoice " + i.id : "Invoice")) + (amount ? " · " + amount : "") }),
              el("small", { text: K.day(i.at) + (i.status ? " · " + i.status : "") }),
            ]),
            i.url && /^https:\/\//.test(i.url) ? el("div", { class: "ra" }, [el("a", { class: "btn ghost sm", href: i.url, target: "_blank", rel: "noopener", text: "View" })]) : null,
          ]));
        });
        pane.appendChild(inv);
      });
    }

    return {
      banner: banner,
      tabs: tabs,
      inviteMail: inviteMail,
      releaseOffline: releaseOffline,
      settings: settings,
      panes: { billing: billing },
    };
  };
})(typeof window !== "undefined" ? window : globalThis);
