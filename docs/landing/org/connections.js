/* The organisation dashboard's company connections (IC-952): the
 * Connections card in Settings.
 *
 * A connection is an API's address and credential, set up once by an owner
 * or admin. The organisation's apps reach it by name (net.connection:<name>)
 * and never hold the credential: Krate adds it to their requests. The card
 * lists each connection (address, what it allows, who and which apps may
 * use it, when its credential was set and how often it was used), and lets
 * an owner or admin make one, replace its credential, change what it allows
 * and who uses it, and remove it. The credential is typed once and never
 * shown again: it is sealed in this browser to the organisation's key
 * (F-316, orgseal.js) and only the sealed blob is sent, so Krate's hub
 * cannot read it; the hub answers only who set it and when.
 *
 * The new-connection form starts from a preset for the systems buyers name
 * first, or from the API's own OpenAPI description (its address, a pasted
 * copy, or a dropped file), read here by /kit/connect.js: the address,
 * what is allowed and the kind of credential are filled in for the admin to
 * review, and a short summary of the API is kept with the connection so
 * Studio's AI writes apps against the real API. Nothing is saved until
 * they press Add connection.
 *
 * dashboard.js owns the page and hands this part what it needs (D). Like the
 * rest of the page it hides only what the role matrix says a role can never
 * use, and says refusals in words.
 */
(function (root) {
  "use strict";

  var METHODS = ["GET", "POST", "PUT", "PATCH", "DELETE"];
  /// The runtime refuses a header value over 4 KiB (adapter-common net.rs),
  /// so a longer credential could never be sent.
  var MAX_VALUE = 4096;
  /// Headers the transport owns: a credential in one would be rewritten or
  /// refused on the way out.
  var TRANSPORT = ["host", "content-length", "transfer-encoding", "connection", "upgrade", "te", "trailer", "keep-alive", "expect"];

  /// A connection's address as the hub keeps it (cloud/worker
  /// org-connections.js normalBase, the same steps): the credential is
  /// sealed for exactly that text.
  function normalBase(raw) {
    var u;
    try { u = new URL(String(raw || "").trim()); } catch (e) { return null; }
    return u.protocol + "//" + u.hostname.toLowerCase() + (u.port ? ":" + u.port : "") + u.pathname.replace(/\/+$/, "");
  }

  /// What was typed, checked here because the hub cannot look inside the
  /// seal: one header (name and value, one line), or a user and password.
  /// Answers `{header, value}` or `{problem: [title, words]}`.
  function credentialOf(typed) {
    if (typed.kind === "basic") {
      var user = typed.user, password = typed.password;
      if (!user || user.indexOf(":") >= 0 || /[\r\n\0]/.test(user + password) || user.length > 256 || password.length > 1024) {
        return { problem: ["Check the user and password", "A user without a colon, and a password, each on one line."] };
      }
      var bytes = new TextEncoder().encode(user + ":" + password);
      var s = "";
      for (var i = 0; i < bytes.length; i += 1) s += String.fromCharCode(bytes[i]);
      var basic = "Basic " + btoa(s);
      if (basic.length > MAX_VALUE) return { problem: ["That user and password are too long", "Krate sends up to " + MAX_VALUE + " characters."] };
      return { header: "Authorization", value: basic };
    }
    var name = String(typed.header || "").trim();
    if (!/^[A-Za-z0-9!#$%&'*+.^_`|~-]{1,64}$/.test(name)) return { problem: ["The header name is one word", "Such as Authorization or X-Api-Key."] };
    if (name.toLowerCase() === "krate-identity") {
      return { problem: ["That header cannot carry a credential", "Krate-Identity is the one Krate uses to tell the API who is calling. Use the header your API names, like Authorization."] };
    }
    if (TRANSPORT.indexOf(name.toLowerCase()) >= 0 || name.toLowerCase().indexOf("proxy-") === 0) {
      return { problem: ["That header cannot carry a credential", name + " is one the connection itself sets. Use the header your API names, like Authorization."] };
    }
    if (!typed.value.trim() || typed.value.length > MAX_VALUE || /[\r\n\0]/.test(typed.value)) {
      return { problem: ["The value is one line", "Up to " + MAX_VALUE + " characters, with no line breaks."] };
    }
    return { header: name, value: typed.value };
  }

  root.KrateOrgConnections = function (D) {
    var K = root.KrateOrg;
    var el = K.el;
    var slug = D.slug;
    var path = function (p) { return "/orgs/" + slug + "/connections" + (p || ""); };
    var nameOf = function () { return (D.S.org && D.S.org.name) || slug; };
    var can = function (action) { return K.can(D.roleOf(), action) === "yes"; };
    var ctx = { what: "set up or change connections", whoCan: "owners and admins" };

    /// The entry side of the credential seam (F-316): the one place a typed
    /// credential becomes what this page sends. It is checked, then sealed
    /// here to the organisation's current public key for this connection's
    /// name, header and address; only the sealed blob and its key version
    /// leave the browser. Answers `{credential}`, `{problem}`, or
    /// `{refused: res}` for the hub's refusal.
    function sealForHub(typed, where) {
      var c = credentialOf(typed);
      if (c.problem) return Promise.resolve({ problem: c.problem });
      return K.api("GET", "/orgs/" + slug + "/keys").then(function (res) {
        if (!res.ok) return { refused: res };
        var cur = res.body && res.body.current;
        if (!cur || !cur.public_key) return { refused: { ok: false, status: 409, body: { error: "no-org-key", message: "" } } };
        var S = root.KrateOrgSeal;
        return S.sealForOrg(cur.public_key, S.connectionContext(slug, where.name, c.header, where.base), c.value).then(function (sealed) {
          return { credential: { kind: typed.kind, header: c.header, v: cur.version, sealed: sealed } };
        });
      }).catch(function () {
        return { problem: ["The credential could not be sealed", "Nothing was sent. Reload the page and try again."] };
      });
    }

    function said(list, last) {
      var l = (list || []).slice();
      if (l.length <= 1) return l.join("");
      return l.slice(0, -1).join(", ") + " " + (last || "and") + " " + l[l.length - 1];
    }

    function scopeWords(c) {
      var who = Array.isArray(c.audience) && c.audience.length ? "Only " + K.groupsSaid(c.audience) : "Everyone in " + nameOf();
      var apps = Array.isArray(c.apps) && c.apps.length ? said(c.apps) : "any of its apps";
      return who + " · " + apps;
    }

    function allowWords(c) {
      var a = c.allow || {};
      var paths = a.paths || ["/"];
      var where = paths.length === 1 && paths[0] === "/" ? "everything under it" : said(paths);
      return said(a.methods || ["GET"], "or") + " on " + where;
    }

    /// Settings: the card, for whoever may see connections.
    function settings(pane) {
      if (!can("connection.view")) return;
      var card = el("div", { class: "card conn", id: "connCard" }, [
        el("h3", { text: "Company connections" }),
        el("p", { text: "An API's address and credential, set up once. Apps in " + nameOf() + " reach it by name and never hold the credential: Krate adds it to their requests." }),
        el("p", { class: "connsafe", text: "Encrypted: Krate cannot read your credentials. Only your members' computers can." }),
        el("p", { class: "loading", text: "Loading" }),
      ]);
      pane.appendChild(card);
      draw(card);
    }

    function draw(card) {
      // The heading and the two lines under it stay.
      while (card.children.length > 3) card.lastChild.remove();
      card.appendChild(el("p", { class: "loading", text: "Loading" }));
      K.api("GET", path()).then(function (res) {
        card.lastChild.remove();
        // A hub from before connections answers 404 here: no card at all,
        // rather than one that can only fail.
        if (!res.ok && res.status === 404 && res.body.error !== "no-such-org") { card.remove(); return; }
        if (!res.ok) { card.appendChild(K.refusalBlock(D.words(res, { what: "see the connections", whoCan: "owners, admins, developers and reviewers" }))); return; }
        var list = (res.body && res.body.connections) || [];
        var rows = el("div", { class: "rows connrows" });
        if (!list.length) {
          rows.appendChild(el("div", { class: "empty" }, [
            el("b", { text: "No connections yet" }),
            el("p", { text: can("connection.manage")
              ? "Add one for an API your apps call, like your finance system. The credential stays with " + nameOf() + "."
              : "An owner or admin of " + nameOf() + " adds them here." }),
          ]));
        }
        list.forEach(function (c) { rows.appendChild(row(card, c)); });
        card.appendChild(rows);
        if (can("connection.manage")) {
          var box = el("div", { class: "connform", hidden: true });
          var add = el("button", { class: "btn dark sm", type: "button", text: "New connection" });
          add.addEventListener("click", function () {
            if (!box.hidden) { box.hidden = true; return; }
            box.textContent = "";
            box.appendChild(createForm(card, box));
            box.hidden = false;
            var first = box.querySelector("input");
            if (first) first.focus();
          });
          card.appendChild(el("div", { class: "row2" }, [add]));
          card.appendChild(box);
        }
        card.appendChild(el("div", { class: "connuse" }, [
          el("span", { text: "An app uses one by naming it in its manifest.toml:" }),
          el("pre", { class: "k", text: "[[capabilities]]\ncap = \"net.connection:" + (list[0] ? list[0].name : "finance-api") + "\"\nrationale = \"Read invoices\"" }),
        ]));
      });
    }

    function row(card, c) {
      var note = el("p", { class: "note", hidden: true });
      var cred = c.credential || {};
      var uses = c.uses || {};
      var again = cred.state === "needs-entry";
      var facts = [
        el("b", {}, [c.name, " ", el("span", { class: "tag blue", text: allowWords(c) })]),
        el("small", { class: "dg", text: c.base }),
        el("small", { text: scopeWords(c) }),
        el("small", { text: c.identity === false ? "Does not tell the API who is calling" : "Tells the API who is calling: each request carries a signed Krate-Identity header naming the person" }),
        c.api ? el("small", { class: "connapi" }, [
          "API: " + c.api.title + (c.api.version ? " " + c.api.version : "") + (c.api.operations ? ", " + c.api.operations + " operation" + (c.api.operations === 1 ? "" : "s") : "") + ", which Studio's AI reads when an app uses it. ",
          el("button", { class: "btn link", type: "button", text: "Save for Studio", onclick: function () { saveForStudio(c, note); } }),
        ]) : null,
        el("small", { text: "Credential: the " + (cred.kind === "basic" ? "user and password" : cred.header + " header") + ", set by " + K.who(cred.set_by).label + " on " + K.day(cred.set_at) +
          " · used " + (uses.today || 0) + (uses.today === 1 ? " time" : " times") + " today, " + (uses.last_30_days || 0) + " in 30 days" }),
        again ? K.refusalBlock({
          title: "Enter this credential again",
          body: "It was saved before Krate encrypted credentials, so Krate deleted it rather than keep a copy it could read. Apps cannot use " + c.name + " until an owner or admin enters it again with Replace credential.",
          action: null,
        }, "warn") : el("small", { class: "connsafe", text: "Encrypted: Krate cannot read this credential." }),
        // The exact terms, for whoever wants them.
        cred.version ? el("details", { class: "conndetails" }, [
          el("summary", { text: "Details" }),
          el("small", { text: "Sealed on the computer it was typed on (in the browser, or by the krate command) to " + nameOf() + "'s organisation key, version " + cred.version +
            ", with HPKE (RFC 9180: X25519, HKDF-SHA256, ChaCha20-Poly1305) for this connection's name, header and address. Krate's hub keeps only the sealed copy; members' computers open it when an app uses the connection." }),
        ]) : null,
        note,
      ];
      var r = el("div", { class: "rw flat connrow" }, [el("div", { class: "t" }, facts)]);
      if (!can("connection.manage")) return r;
      var box = el("div", { class: "connform", hidden: true });
      var open = function (make) {
        return function () {
          var showing = !box.hidden && box.dataset.what === make.name;
          box.textContent = "";
          box.hidden = showing;
          if (showing) return;
          box.dataset.what = make.name;
          box.appendChild(make(card, c, box));
          var first = box.querySelector("input");
          if (first) first.focus();
        };
      };
      r.appendChild(el("div", { class: "ra manage" }, [
        el("button", { class: "btn ghost sm", type: "button", text: "Replace credential", onclick: open(credentialForm) }),
        el("button", { class: "btn ghost sm", type: "button", text: "Change", onclick: open(changeForm) }),
        el("button", { class: "btn quiet sm", type: "button", text: "Remove", onclick: function () { remove(card, c); } }),
      ]));
      r.appendChild(box);
      return r;
    }

    /// The connection as Studio's AI reads it, as a file: Studio in a tab
    /// adds it to a build by itself; on a computer, attach this file to
    /// the request.
    function saveForStudio(c, note) {
      K.api("GET", path("/" + encodeURIComponent(c.name) + "/api")).then(function (res) {
        note.hidden = false;
        if (!res.ok) { note.textContent = D.words(res, { what: "read this connection's API", whoCan: "owners, admins, developers and reviewers" }).body; return; }
        var text = root.KrateConnect.contextText(res.body, nameOf());
        var blob = new root.Blob([text], { type: "text/markdown" });
        var href = root.URL.createObjectURL(blob);
        var a = root.document.createElement("a");
        a.href = href;
        a.setAttribute("download", "connection-" + c.name + ".md");
        if (a.click) a.click();
        note.textContent = "Saved connection-" + c.name + ".md. Attach it in Studio when you ask for an app that uses " + c.name + ". Studio in the browser adds it by itself.";
      });
    }

    // ------------------------------------------------------------- parts

    function fld(label, input, hint) {
      return el("label", { class: "fld" }, [el("span", { text: label }), input, hint ? el("small", { text: hint }) : null]);
    }

    /// The credential: one header, or a user and password. `set(auth)`
    /// shapes it for what a preset or a description asks for: the kind,
    /// the header's name, a prefix the value is sent with (Bearer), and
    /// the words for what to paste.
    function credentialPart() {
      var kind = "header";
      var prefix = "";
      var seg = el("div", { class: "seg", role: "group", "aria-label": "Kind of credential" });
      var headerName = el("input", { class: "inp", name: "headerName", value: "Authorization", autocomplete: "off", spellcheck: "false", autocapitalize: "none" });
      var value = el("input", { class: "inp", name: "credValue", type: "password", autocomplete: "new-password", placeholder: "Bearer ...", spellcheck: "false" });
      var user = el("input", { class: "inp", name: "credUser", autocomplete: "off", spellcheck: "false", autocapitalize: "none" });
      var password = el("input", { class: "inp", name: "credPassword", type: "password", autocomplete: "new-password" });
      var SEALED = "Encrypted on this computer before it is sent: Krate cannot read it. It is never shown again, here or to an app. A server that echoes request headers back would show it to the app.";
      var valueFld = fld("Value", value, SEALED);
      var userFld = fld("User", user);
      var passwordFld = fld("Password", password, "Encrypted on this computer before it is sent: Krate cannot read it. It is never shown again.");
      var asHeader = el("div", { class: "form" }, [
        fld("Header", headerName, "Usually Authorization, or the one your API names, like X-Api-Key."),
        valueFld,
      ]);
      var asBasic = el("div", { class: "form", hidden: true }, [userFld, passwordFld]);
      var pick = function (k) {
        kind = k;
        [].forEach.call(seg.children, function (x) { x.setAttribute("aria-pressed", x.dataset.kind === kind ? "true" : "false"); });
        asHeader.hidden = kind !== "header";
        asBasic.hidden = kind !== "basic";
      };
      var label = function (f, text) { f.children[0].textContent = text; };
      var small = function (f, text) { f.children[2].textContent = text; };
      [["header", "A header"], ["basic", "User and password"]].forEach(function (p) {
        var b = el("button", { type: "button", "aria-pressed": p[0] === kind ? "true" : "false", text: p[1] });
        b.dataset.kind = p[0];
        b.addEventListener("click", function () { prefix = ""; pick(p[0]); });
        seg.appendChild(b);
      });
      return {
        node: el("div", { class: "fld" }, [el("span", { text: "Credential" }), seg, asHeader, asBasic]),
        set: function (auth) {
          auth = auth || { kind: "header", header: "Authorization" };
          pick(auth.kind === "basic" ? "basic" : "header");
          prefix = auth.prefix || "";
          if (auth.kind !== "basic") headerName.value = auth.header || "Authorization";
          label(valueFld, auth.secret ? "Value: " + auth.secret : "Value");
          value.setAttribute("placeholder", prefix ? "Paste the token" : headerName.value === "Authorization" ? "Bearer ..." : "Paste the value");
          small(valueFld, (prefix ? "Paste it alone: Krate sends it after " + prefix.trim() + " and a space. " : "") + SEALED);
          label(userFld, auth.user || "User");
          label(passwordFld, auth.kind === "basic" && auth.secret ? auth.secret : "Password");
        },
        /// What was typed, for sealForHub alone: never a request body.
        typed: function () {
          if (kind === "basic") return user.value.trim() ? { kind: "basic", user: user.value.trim(), password: password.value } : null;
          var v = value.value.trim();
          if (!v) return null;
          if (prefix && v.toLowerCase().indexOf(prefix.toLowerCase()) !== 0) v = prefix + v;
          return { kind: "header", header: headerName.value.trim(), value: v };
        },
        clear: function () { value.value = ""; password.value = ""; },
      };
    }

    /// What it allows: methods and paths.
    function allowPart(cur) {
      cur = cur || { methods: ["GET"], paths: ["/"] };
      var boxes = METHODS.map(function (m) {
        var i = el("input", { type: "checkbox", name: "method", value: m });
        i.checked = (cur.methods || []).indexOf(m) >= 0;
        return el("label", { class: "chk" }, [i, el("span", { text: m })]);
      });
      var paths = el("input", { class: "inp", name: "paths", autocomplete: "off", spellcheck: "false", autocapitalize: "none",
        value: (cur.paths || []).filter(function (p) { return p !== "/"; }).join(", "), placeholder: "Everything under the address" });
      var methodsHint = el("small", { text: "GET reads. Tick more only if the apps must change things." });
      var changed = null;
      boxes.forEach(function (b) { b.querySelector("input").addEventListener("change", function () { if (changed) changed(); }); });
      var shown = function (list) { return (list || []).filter(function (p) { return p !== "/"; }).join(", "); };
      return {
        node: el("div", { class: "form" }, [
          el("div", { class: "fld" }, [el("span", { text: "Apps may" }), el("div", { class: "mchecks" }, boxes), methodsHint]),
          fld("Paths", paths, "Under the address, comma-separated, like /invoices, /rates. Empty is all of it."),
        ]),
        /// Fill it: the methods ticked and the paths, and what the API has.
        set: function (methods, list) {
          boxes.forEach(function (b) { var i = b.querySelector("input"); i.checked = methods.indexOf(i.value) >= 0; });
          paths.value = shown(list);
        },
        ticked: function () {
          return boxes.map(function (b) { return b.querySelector("input"); }).filter(function (i) { return i.checked; }).map(function (i) { return i.value; });
        },
        /// The paths field, when nobody has typed in it since `list`.
        untouched: function (list) { return paths.value === shown(list); },
        onChange: function (f) { changed = f; },
        body: function () {
          var m = boxes.map(function (b) { return b.querySelector("input"); }).filter(function (i) { return i.checked; }).map(function (i) { return i.value; });
          var p = paths.value.split(/[,\s]+/).map(function (x) { return x.trim(); }).filter(Boolean);
          return { methods: m, paths: p.length ? p : ["/"] };
        },
      };
    }

    /// Who may use it, and which apps.
    function scopePart(cur) {
      cur = cur || {};
      var groupsBox = el("div", { class: "gchecks" }, [el("p", { class: "sub", text: "Loading groups" })]);
      var checks = [];
      var everyone = !(Array.isArray(cur.audience) && cur.audience.length);
      var whoSeg = el("div", { class: "seg", role: "group", "aria-label": "Who may use it" });
      var setWho = function (all) {
        everyone = all;
        [].forEach.call(whoSeg.children, function (x) { x.setAttribute("aria-pressed", (x.dataset.all === "1") === all ? "true" : "false"); });
        groupsBox.hidden = all;
      };
      [["1", "Everyone"], ["0", "Some groups"]].forEach(function (p) {
        var b = el("button", { type: "button", text: p[1] });
        b.dataset.all = p[0];
        b.addEventListener("click", function () { setWho(p[0] === "1"); });
        whoSeg.appendChild(b);
      });
      K.api("GET", "/orgs/" + slug + "/groups").then(function (res) {
        groupsBox.textContent = "";
        var groups = res.ok && res.body && Array.isArray(res.body.groups) ? res.body.groups : [];
        if (!groups.length) groupsBox.appendChild(el("p", { class: "sub", text: "No groups yet. Make one on the People tab." }));
        groups.forEach(function (g) {
          var i = el("input", { type: "checkbox", name: "group", value: g.name });
          i.checked = Array.isArray(cur.audience) && cur.audience.indexOf(g.name) >= 0;
          checks.push(i);
          groupsBox.appendChild(el("label", { class: "chk" }, [i, el("span", { text: g.name + " (" + g.members.length + ")" })]));
        });
      });
      setWho(everyone);
      var apps = el("input", { class: "inp", name: "apps", autocomplete: "off", spellcheck: "false", autocapitalize: "none",
        value: Array.isArray(cur.apps) ? cur.apps.join(", ") : "", placeholder: "Any of the organisation's apps" });
      return {
        node: el("div", { class: "form" }, [
          el("div", { class: "fld" }, [el("span", { text: "Who may use it" }), whoSeg, groupsBox]),
          fld("Which apps", apps, "App ids from their manifest, like com.acme.ledger, comma-separated. Empty is any of " + nameOf() + "'s apps."),
        ]),
        body: function () {
          var ids = apps.value.split(/[,\s]+/).map(function (x) { return x.trim().toLowerCase(); }).filter(Boolean);
          var picked = checks.filter(function (i) { return i.checked; }).map(function (i) { return i.value; });
          return {
            audience: everyone ? { everyone: true } : { groups: picked },
            apps: ids.length ? { ids: ids } : { any: true },
            pickedNone: !everyone && !picked.length,
          };
        },
      };
    }

    /// "Tell the API who is calling": on unless turned off. When on, each
    /// request also carries a short-lived Krate-Identity token, signed by
    /// Krate, naming the person, the app and the release, so the API can
    /// log who did what. The credential still decides access.
    function identityPart(cur) {
      var box = el("input", { type: "checkbox", name: "identity" });
      box.checked = cur !== false;
      return {
        node: el("div", { class: "fld" }, [
          el("label", { class: "chk" }, [box, el("span", { text: "Tell the API who is calling" })]),
          el("small", { text: "Each request also carries a Krate-Identity header, signed by Krate, naming the person, the app and its version. Your API can log it and check it; it needs no sign-in work. Docs: krate.tech/docs/organisations.html#telling-the-api-who-is-calling" }),
        ]),
        value: function () { return box.checked; },
      };
    }

    function outBox() { return el("div", { class: "connout" }); }
    function hint(out, t, b) { out.textContent = ""; out.appendChild(K.refusalBlock({ title: t, body: b, action: null }, "warn")); }

    function saving(button, label) {
      button.disabled = true;
      var was = button.textContent;
      button.textContent = label;
      return function () { button.disabled = false; button.textContent = was; };
    }

    // ------------------------------------------------------------- forms

    /// "Start from": one chip per preset. Choosing one fills the form; the
    /// admin can still change everything.
    function presetPart(onPick) {
      var C = root.KrateConnect;
      var row = el("div", { class: "connpresets", role: "group", "aria-label": "Start from" });
      (C ? C.PRESETS : []).forEach(function (p) {
        var b = el("button", { class: "chip", type: "button", "aria-pressed": "false", text: p.label });
        b.dataset.preset = p.id;
        if (p.what) b.setAttribute("title", p.label + ": " + p.what);
        b.addEventListener("click", function () {
          [].forEach.call(row.children, function (x) { x.setAttribute("aria-pressed", x === b ? "true" : "false"); });
          onPick(p);
        });
        row.appendChild(b);
      });
      return el("div", { class: "fld" }, [el("span", { text: "Start from" }), row]);
    }

    /// The API's own description: its address, or the text pasted or a
    /// file dropped. Read here; `onRead(result)` fills the form.
    function importPart(onRead) {
      var C = root.KrateConnect;
      var url = el("input", { class: "inp", name: "specUrl", type: "url", inputmode: "url", autocomplete: "off", spellcheck: "false", autocapitalize: "none", placeholder: "https://api.acme.com/openapi.json" });
      var paste = el("textarea", { class: "inp", name: "specText", rows: "4", spellcheck: "false", autocapitalize: "none", placeholder: "Or paste the description here, or drop its file (.json or .yaml)" });
      var file = el("input", { type: "file", accept: ".json,.yaml,.yml,application/json,application/yaml,text/yaml", hidden: true });
      var out = outBox();
      var readBtn = el("button", { class: "btn ghost sm", type: "button", text: "Read it" });
      var useBtn = el("button", { class: "btn ghost sm", type: "button", text: "Use what I pasted" });
      var fileBtn = el("button", { class: "btn quiet sm", type: "button", text: "Choose a file", onclick: function () { file.click(); } });
      var said = function (title, body, extra) {
        out.textContent = "";
        var block = K.refusalBlock({ title: title, body: body, action: null }, "warn");
        if (extra) block.children[0].appendChild(el("div", { class: "row2" }, extra));
        out.appendChild(block);
      };
      var read = function (text, from) {
        var r = C.parse(text);
        if (r.problem) return said(r.problem[0], r.problem[1]);
        var made = C.fromSpec(r.spec, { url: from });
        if (made.problem) return said(made.problem[0], made.problem[1]);
        out.textContent = "";
        onRead(made);
      };
      var fromText = function (text, from) {
        if (!text) return said("Nothing to read", "Paste the description, or drop its file here.");
        read(text, from || null);
      };
      readBtn.addEventListener("click", function () {
        var u = url.value.trim();
        if (!/^https:\/\/[^\s<>]+$/.test(u)) return said("The address starts with https://", "Where the API publishes its description, often ending in openapi.json or swagger.json.");
        var done = saving(readBtn, "Reading");
        var cors = function () {
          done();
          said("That site does not let this page read it", "Many API sites only let their own pages read their description. Open it, copy all of it, and paste it below.", [
            el("a", { class: "btn ghost sm", href: u, target: "_blank", rel: "noopener noreferrer", text: "Open it in a new tab" }),
            el("button", { class: "btn quiet sm", type: "button", text: "Paste it instead", onclick: function () { paste.focus(); } }),
          ]);
        };
        root.fetch(u, { credentials: "omit", redirect: "follow" }).then(function (res) {
          if (!res.ok) {
            done();
            return said("That address answered " + res.status, "Check it opens the description itself in a new tab, or paste the description below.");
          }
          var size = Number((res.headers && res.headers.get && res.headers.get("content-length")) || 0);
          if (size > 16 * 1024 * 1024) { done(); return said("That description is too big", "Krate reads descriptions up to 16 MB."); }
          return res.text().then(function (text) { done(); read(text, u); });
        }, cors);
      });
      useBtn.addEventListener("click", function () { fromText(paste.value.trim(), null); });
      var readFile = function (f) {
        if (!f) return;
        if (f.size > 16 * 1024 * 1024) return said("That file is too big", "Krate reads descriptions up to 16 MB.");
        f.text().then(function (text) { paste.value = ""; fromText(text, null); }, function () { said("That file could not be read", "Try again, or paste its contents instead."); });
      };
      file.addEventListener("change", function () { readFile(file.files && file.files[0]); file.value = ""; });
      paste.addEventListener("dragover", function (e) { e.preventDefault(); });
      paste.addEventListener("drop", function (e) {
        var f = e.dataTransfer && e.dataTransfer.files && e.dataTransfer.files[0];
        if (!f) return;
        e.preventDefault();
        readFile(f);
      });
      return el("details", { class: "connimport" }, [
        el("summary", { text: "Import its OpenAPI description" }),
        el("div", { class: "form" }, [
          el("small", { class: "sub", text: "OpenAPI 3 or Swagger 2, JSON or YAML. Krate fills in the address, what apps may do and the kind of credential, for you to check before you add it." }),
          el("div", { class: "fld" }, [el("span", { text: "Its address" }), el("div", { class: "connfetch" }, [url, readBtn])]),
          paste,
          el("div", { class: "row2" }, [useBtn, fileBtn]),
          file, out,
        ]),
      ]);
    }

    function slugOf(title) {
      var s = String(title || "").toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 39).replace(/-+$/, "");
      return /^[a-z][a-z0-9-]{1,38}$/.test(s) ? s : "";
    }

    function createForm(card, box) {
      var C = root.KrateConnect;
      var name = el("input", { class: "inp", name: "connName", autocomplete: "off", spellcheck: "false", autocapitalize: "none", placeholder: "finance-api" });
      var base = el("input", { class: "inp", name: "connBase", type: "url", autocomplete: "off", spellcheck: "false", autocapitalize: "none", placeholder: "https://api.acme.com/finance" });
      var cred = credentialPart();
      var allow = allowPart();
      var scope = scopePart();
      var who = identityPart(true);
      var out = outBox();
      // What the form was filled from: the preset, and the description's
      // operations and summary.
      var from = { preset: null, ops: null, paths: null, api: null, made: null };
      var guide = el("div", { class: "connguide", hidden: true });
      var review = el("div", { class: "connreview", hidden: true });
      var suggested = "";
      var suggest = function (n) {
        if (!n) return;
        if (!name.value.trim() || name.value.trim() === suggested) { name.value = n; suggested = n; }
      };
      var showGuide = function (p) {
        guide.textContent = "";
        guide.hidden = !(p.note || p.docs);
        if (p.note) guide.appendChild(el("p", { text: p.note }));
        if (p.docs) guide.appendChild(el("a", { class: "btn ghost sm", href: p.docs, target: "_blank", rel: "noopener noreferrer", text: "How to make the credential in " + p.label }));
      };
      var presetApi = function (p) {
        return p && p.ai ? { title: (p.label + " " + (p.what || "")).trim(), version: "", source: "preset:" + p.id, notes: p.ai, operations: [], more: 0 } : null;
      };
      var presets = presetPart(function (p) {
        from = { preset: p, ops: null, paths: null, api: presetApi(p), made: null };
        review.hidden = true;
        suggest(p.name);
        base.value = p.base;
        base.setAttribute("placeholder", p.basePlaceholder || "https://api.acme.com/finance");
        cred.set(p.auth);
        allow.set(p.methods, p.paths);
        showGuide(p);
      });
      var imported = importPart(function (r) {
        var keepNotes = from.preset && from.preset.ai;
        from = { preset: from.preset, ops: r.ops, paths: r.paths, api: keepNotes ? Object.assign({}, r.api, { notes: from.preset.ai }) : r.api, made: r };
        if (r.base) base.value = r.base;
        suggest(slugOf(r.title));
        if (r.auth) cred.set(r.auth);
        allow.set(r.ticked, r.paths);
        review.textContent = "";
        review.hidden = false;
        var a = r.api;
        review.appendChild(el("b", { text: "Read " + a.title + (a.version ? " " + a.version : "") + ": " + r.ops.length + " operation" + (r.ops.length === 1 ? "" : "s") }));
        review.appendChild(el("p", { text: "Check the address, what apps may do and the credential below, then add it. Nothing is saved until you do." }));
        r.notes.forEach(function (n) { review.appendChild(el("small", { text: n })); });
        if (a.more) review.appendChild(el("small", { text: "Studio's AI reads the first " + a.operations.length + " operations; the other " + a.more + " did not fit." }));
      });
      // Ticking a method allows the paths of that method's operations,
      // while nobody has typed their own.
      allow.onChange(function () {
        if (!from.ops || !allow.untouched(from.paths)) return;
        var p = C.pathsFor(from.ops, allow.ticked());
        from.paths = p.paths.length ? p.paths : ["/"];
        allow.set(allow.ticked(), from.paths);
      });
      var save = el("button", { class: "btn dark", type: "button", text: "Add connection" });
      var cancel = el("button", { class: "btn ghost", type: "button", text: "Cancel", onclick: function () { box.hidden = true; } });
      save.addEventListener("click", function () {
        var n = name.value.trim().toLowerCase();
        var b = base.value.trim();
        if (!/^[a-z][a-z0-9-]{1,38}$/.test(n)) return hint(out, "The name is short and plain", "2 to 39 lower-case letters, digits and dashes, like finance-api. Apps ask for it by this name.");
        var hole = /<([^>]+)>/.exec(b);
        if (hole) return hint(out, "Put your own " + hole[1] + " in the address", "Replace <" + hole[1] + "> with yours, as it appears in the address you sign in to.");
        var local = /^(127\.0\.0\.1|localhost)$/.test(location.hostname) && /^http:\/\/(127\.0\.0\.1|localhost)(:\d+)?\//.test(b + "/");
        if (!/^https:\/\/[^\s<>]+$/.test(b) && !local) return hint(out, "The address starts with https://", "A host and a path, like https://api.acme.com/finance. Requests under it get the credential.");
        var c = cred.typed();
        if (!c) return hint(out, "The credential is missing", "Paste the header's value, or the user and password, your API gave you.");
        var a = allow.body();
        if (!a.methods.length) return hint(out, "Tick at least one thing apps may do", "GET alone lets them read.");
        var s = scope.body();
        if (s.pickedNone) return hint(out, "Pick at least one group", "Or choose Everyone.");
        var where = { name: n, base: normalBase(b) };
        if (!where.base) return hint(out, "The address starts with https://", "A host and a path, like https://api.acme.com/finance.");
        out.textContent = "";
        var done = saving(save, "Encrypting and adding");
        sealForHub(c, where).then(function (sealed) {
          if (sealed.problem || sealed.refused) return sealed;
          var body = { name: n, base: where.base, allow: a, audience: s.audience, apps: s.apps, identity: who.value(), credential: sealed.credential };
          // Summarised again for what it now allows: those operations first.
          if (from.made) from.api = C.summary(from.made.spec, from.made.ops, from.made.url, a, from.api.notes);
          if (from.api) body.api = from.api;
          return K.api("POST", path(), body, { key: K.key() });
        }).then(function (res) {
          done();
          if (res.problem) return hint(out, res.problem[0], res.problem[1]);
          cred.clear();
          res = res.refused || res;
          if (!res.ok) { out.appendChild(K.refusalBlock(D.words(res, ctx), "bad")); return; }
          D.toast("The " + n + " connection is ready. Apps that ask for net.connection:" + n + " can use it.");
          draw(card);
        });
      });
      return el("div", { class: "connbox" }, [
        el("b", { text: "New connection" }),
        presets, guide, imported, review,
        el("div", { class: "form" }, [fld("Name", name, "Apps ask for it as net.connection:<name>."), fld("Address", base, "Requests under this address get the credential. Anything else never does.")]),
        cred.node, allow.node, scope.node, who.node, out,
        el("div", { class: "row2" }, [save, cancel]),
      ]);
    }

    function credentialForm(card, c, box) {
      var cred = credentialPart();
      var out = outBox();
      var save = el("button", { class: "btn dark", type: "button", text: "Replace credential" });
      var cancel = el("button", { class: "btn ghost", type: "button", text: "Cancel", onclick: function () { box.hidden = true; } });
      save.addEventListener("click", function () {
        var typed = cred.typed();
        if (!typed) return hint(out, "The new credential is missing", "Paste the new value. The old one is deleted when you save.");
        out.textContent = "";
        var done = saving(save, "Encrypting and replacing");
        sealForHub(typed, { name: c.name, base: c.base }).then(function (sealed) {
          if (sealed.problem || sealed.refused) return sealed;
          return K.api("PUT", path("/" + encodeURIComponent(c.name) + "/credential"), { credential: sealed.credential }, { key: K.key() });
        }).then(function (res) {
          done();
          if (res.problem) return hint(out, res.problem[0], res.problem[1]);
          cred.clear();
          res = res.refused || res;
          if (!res.ok) { out.appendChild(K.refusalBlock(D.words(res, ctx), "bad")); return; }
          D.toast(c.name + " has a new credential. Apps get it the next time they open.");
          draw(card);
        });
      });
      return el("div", { class: "connbox" }, [
        el("b", { text: "Replace the credential of " + c.name }),
        el("p", { class: "sub", text: "The old one is deleted. Every app that uses it gets the new one the next time it opens." }),
        cred.node, out, el("div", { class: "row2" }, [save, cancel]),
      ]);
    }

    function changeForm(card, c, box) {
      var C = root.KrateConnect;
      var allow = allowPart(c.allow);
      var scope = scopePart(c);
      var who = identityPart(c.identity);
      var out = outBox();
      var from = { ops: null, paths: null, api: undefined };
      var review = el("div", { class: "connreview", hidden: true });
      var drop = c.api ? el("input", { type: "checkbox", name: "dropApi" }) : null;
      var imported = importPart(function (r) {
        from = { ops: r.ops, paths: r.paths, api: r.api, made: r };
        allow.set(r.ticked, r.paths);
        review.textContent = "";
        review.hidden = false;
        review.appendChild(el("b", { text: "Read " + r.api.title + (r.api.version ? " " + r.api.version : "") + ": " + r.ops.length + " operation" + (r.ops.length === 1 ? "" : "s") }));
        review.appendChild(el("p", { text: "What apps may do is filled in from it. Check it, then save. Nothing changes until you do." }));
        if (r.base && normalBase(r.base) !== c.base) review.appendChild(el("small", { text: "Its address is " + r.base + ". This connection's stays " + c.base + "." }));
        r.notes.filter(function (n) { return !/address/.test(n); }).forEach(function (n) { review.appendChild(el("small", { text: n })); });
      });
      allow.onChange(function () {
        if (!from.ops || !allow.untouched(from.paths)) return;
        var p = C.pathsFor(from.ops, allow.ticked());
        from.paths = p.paths.length ? p.paths : ["/"];
        allow.set(allow.ticked(), from.paths);
      });
      var save = el("button", { class: "btn dark", type: "button", text: "Save changes" });
      var cancel = el("button", { class: "btn ghost", type: "button", text: "Cancel", onclick: function () { box.hidden = true; } });
      save.addEventListener("click", function () {
        var a = allow.body();
        if (!a.methods.length) return hint(out, "Tick at least one thing apps may do", "GET alone lets them read.");
        var s = scope.body();
        if (s.pickedNone) return hint(out, "Pick at least one group", "Or choose Everyone.");
        out.textContent = "";
        var done = saving(save, "Saving");
        var body = { allow: a, audience: s.audience, apps: s.apps, identity: who.value() };
        if (from.made) from.api = C.summary(from.made.spec, from.made.ops, from.made.url, a);
        if (from.api) body.api = from.api;
        else if (drop && drop.checked) body.api = null;
        K.api("PUT", path("/" + encodeURIComponent(c.name)), body, { key: K.key() }).then(function (res) {
          done();
          if (!res.ok) { out.appendChild(K.refusalBlock(D.words(res, ctx), "bad")); return; }
          D.toast(c.name + " is changed. Apps follow it the next time they open.");
          draw(card);
        });
      });
      return el("div", { class: "connbox" }, [
        el("b", { text: "Change " + c.name }),
        el("p", { class: "sub", text: "Its address stays " + c.base + ". To send the credential somewhere else, remove this connection and add it again with the new address." }),
        imported, review,
        drop ? el("label", { class: "chk" }, [drop, el("span", { text: "Remove its API description" })]) : null,
        allow.node, scope.node, who.node, out, el("div", { class: "row2" }, [save, cancel]),
      ]);
    }

    function remove(card, c) {
      var key = K.key();
      D.confirmSheet({
        title: "Remove " + c.name + "?",
        body: "Its credential is deleted from Krate. Apps that use it are refused it from the next time they open, and say so.",
        points: [c.base, scopeWords(c)],
        yes: "Remove it",
        busy: "Removing",
        ctx: ctx,
        act: function () { return K.api("DELETE", path("/" + encodeURIComponent(c.name)), null, { key: key }); },
        done: function () { D.toast(c.name + " is removed, with its credential."); draw(card); },
      });
    }

    return { settings: settings };
  };
})(typeof window !== "undefined" ? window : globalThis);
