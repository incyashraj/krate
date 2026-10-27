/* window.KrateConnect -- an API the company already has, made a company
 * connection in minutes (SMART-FIXES section 4).
 *
 *   parse(text)              -> {spec} | {problem: [title, words]}
 *        an OpenAPI 3 or Swagger 2 description, as JSON or as YAML (the
 *        block YAML these descriptions are written in; anchors are refused
 *        with words that ask for the JSON version)
 *   fromSpec(spec, {url})    -> {problem} | {base, needs, methods, ticked,
 *                                ops, paths, widened, auth, api, notes}
 *        what the connection form is filled with: the address from
 *        servers (or Swagger's schemes, host and basePath), every method
 *        the description uses with GET alone ticked, the shortest path
 *        prefixes that cover its operations, the kind of credential its
 *        securitySchemes name, and the summary Studio's AI reads
 *   pathsFor(ops, methods)   -> {paths, widened}
 *        the prefixes for the ticked methods' operations only: each path's
 *        part before its first {parameter}, the ones under another
 *        dropped, never anything outside the address
 *   summary(spec, ops, url)  -> {title, version, source, operations, more}
 *        operation id, method, path, a short summary and the query
 *        parameters' names, under MAX_API bytes as JSON
 *   contextText(conn, org)   -> the Markdown a build is handed about one
 *        connection: how to declare it, its address, what it allows, and
 *        the operations it allows from its description
 *   PRESETS                  -> the systems buyers name first
 *
 * Nothing here talks to the network: the console fetches a description's
 * address itself and hands the text here. docs/landing/studio/
 * connect-test.mjs runs this file as it ships.
 */
(function (root) {
  "use strict";

  /// The most a connection's API summary may be, as JSON: the hub refuses
  /// more (cloud/worker org-connections.js normalApi).
  var MAX_API = 24 * 1024;
  /// The most a pasted or fetched description may be.
  var MAX_SPEC = 16 * 1024 * 1024;
  /// The most path prefixes a connection may allow (the hub's MAX_PATHS).
  var MAX_PATHS = 50;
  var VERBS = ["get", "put", "post", "delete", "patch", "head", "options"];
  /// The methods the console offers; the hub knows HEAD and OPTIONS too.
  var FORM_METHODS = ["GET", "POST", "PUT", "PATCH", "DELETE"];
  /// A path prefix the hub can match exactly (normalAllow).
  var GOOD_PATH = /^\/[^%?#\\\s]{0,199}$/;

  function problem(title, words) { return { problem: [title, words] }; }

  function clip(s, n) {
    s = String(s == null ? "" : s).replace(/[\u0000-\u001f\u007f]+/g, " ").replace(/\s+/g, " ").trim();
    return s.length > n ? s.slice(0, n - 1).replace(/\s+\S*$/, "") + "…" : s;
  }

  // ------------------------------------------------------------- YAML
  //
  // The block YAML API descriptions are written in: mappings, sequences,
  // plain, quoted and block (| and >) scalars, one-line flow [..] and {..},
  // comments. Anchors, aliases, merges and tags are refused, with words.

  function yamlRefusal(line, what) {
    var e = new Error(what + (line ? " (line " + line + ")" : ""));
    e.yaml = true;
    return e;
  }

  function stripComment(s) {
    var q = null;
    for (var i = 0; i < s.length; i += 1) {
      var c = s[i];
      if (q) {
        if (c === "\\" && q === '"') { i += 1; continue; }
        if (c === q) q = null;
      } else if (c === '"' || c === "'") {
        if (i === 0 || /[\s\[{,:]/.test(s[i - 1])) q = c;
      } else if (c === "#" && (i === 0 || /\s/.test(s[i - 1]))) {
        return s.slice(0, i).replace(/\s+$/, "");
      }
    }
    return s.replace(/\s+$/, "");
  }

  function plainScalar(s) {
    if (s === "" || s === "~" || s === "null" || s === "Null" || s === "NULL") return null;
    if (/^(true|True|TRUE)$/.test(s)) return true;
    if (/^(false|False|FALSE)$/.test(s)) return false;
    if (/^[-+]?(0|[1-9][0-9]*)$/.test(s)) return Number(s);
    if (/^[-+]?([0-9]+\.[0-9]*|\.[0-9]+)([eE][-+]?[0-9]+)?$/.test(s)) return Number(s);
    return s;
  }

  /// One flow or quoted value starting at s[i]: answers [value, next i].
  function flowValue(s, i, line) {
    while (s[i] === " ") i += 1;
    var c = s[i];
    if (c === '"') {
      var j = i + 1, out = "";
      for (; j < s.length; j += 1) {
        if (s[j] === "\\") { out += s[j] + s[j + 1]; j += 1; continue; }
        if (s[j] === '"') break;
        out += s[j];
      }
      if (j >= s.length) throw yamlRefusal(line, "a quoted value is not closed");
      var v;
      try { v = JSON.parse('"' + out.replace(/\\x([0-9a-fA-F]{2})/g, "\\u00$1").replace(/\\ /g, " ").replace(/\t/g, "\\t") + '"'); } catch (e) { v = out; }
      return [v, j + 1];
    }
    if (c === "'") {
      var k = i + 1, txt = "";
      for (; k < s.length; k += 1) {
        if (s[k] === "'") { if (s[k + 1] === "'") { txt += "'"; k += 1; continue; } break; }
        txt += s[k];
      }
      if (k >= s.length) throw yamlRefusal(line, "a quoted value is not closed");
      return [txt, k + 1];
    }
    if (c === "[") {
      var arr = [];
      i += 1;
      for (;;) {
        while (s[i] === " ") i += 1;
        if (s[i] === "]") return [arr, i + 1];
        if (i >= s.length) throw yamlRefusal(line, "a [ list is not closed");
        var item = flowValue(s, i, line);
        arr.push(item[0]);
        i = item[1];
        while (s[i] === " ") i += 1;
        if (s[i] === ",") i += 1;
      }
    }
    if (c === "{") {
      var obj = {};
      i += 1;
      for (;;) {
        while (s[i] === " " || s[i] === ",") i += 1;
        if (s[i] === "}") return [obj, i + 1];
        if (i >= s.length) throw yamlRefusal(line, "a { map is not closed");
        var key;
        if (s[i] === '"' || s[i] === "'") {
          var kq = flowValue(s, i, line);
          key = String(kq[0]);
          i = kq[1];
        } else {
          var m = /^[^:,}]*/.exec(s.slice(i))[0];
          key = m.trim();
          i += m.length;
        }
        while (s[i] === " ") i += 1;
        if (s[i] === ":") {
          var val = flowValue(s, i + 1, line);
          obj[key] = val[0];
          i = val[1];
        } else {
          obj[key] = null;
        }
      }
    }
    // A plain scalar inside a flow: up to the next , ] or }.
    var p = /^[^,\]}]*/.exec(s.slice(i))[0];
    return [plainScalar(p.trim()), i + p.length];
  }

  /// Is a flow or quoted value still open at the end of `s`?
  function unclosed(s) {
    var depth = 0, q = null;
    for (var k = 0; k < s.length; k += 1) {
      var c = s[k];
      if (q) { if (c === "\\" && q === '"') k += 1; else if (c === q) q = null; continue; }
      if ((c === '"' || c === "'") && (k === 0 || /[\s\[{,:]/.test(s[k - 1]))) q = c;
      else if (c === "[" || c === "{") depth += 1;
      else if (c === "]" || c === "}") depth -= 1;
    }
    return depth > 0 || q !== null;
  }

  function inlineValue(s, line) {
    if (/^[&*!]/.test(s)) throw yamlRefusal(line, "this YAML uses anchors, aliases or tags (& * !)");
    if (s[0] === '"' || s[0] === "'" || s[0] === "[" || s[0] === "{") {
      var v = flowValue(s, 0, line);
      var rest = s.slice(v[1]).trim();
      if (rest && (s[0] === "[" || s[0] === "{")) throw yamlRefusal(line, "something follows a closed [ or {");
      return rest && (s[0] === '"' || s[0] === "'") ? String(v[0]) + " " + rest : v[0];
    }
    return plainScalar(s);
  }

  function parseYaml(text) {
    var raw = String(text).replace(/^﻿/, "").replace(/\r\n?/g, "\n").split("\n");
    var lines = [];
    for (var n = 0; n < raw.length; n += 1) {
      var r = raw[n];
      var text2 = stripComment(r);
      var t = text2.trim();
      if (!t || t === "---" || t === "..." || /^%/.test(t)) { lines.push({ raw: r, blank: true, no: n + 1 }); continue; }
      lines.push({ raw: r, indent: text2.length - text2.replace(/^ +/, "").length, text: t, no: n + 1 });
    }
    var i = 0;
    function skip() { while (i < lines.length && lines[i].blank) i += 1; }
    function cur() { skip(); return lines[i]; }

    function isSeq(l) { return l.text === "-" || l.text.indexOf("- ") === 0; }

    /// A key and the rest of its line, or null when the line is no entry.
    function entry(t) {
      var m;
      if (t[0] === '"' || t[0] === "'") {
        var q = flowValue(t, 0, 0);
        var after = t.slice(q[1]);
        m = /^\s*:(\s+|$)/.exec(after);
        if (!m) return null;
        return { key: String(q[0]), rest: after.slice(m[0].length).trim() };
      }
      m = /^([^#\s][^:]*?|[^#\s]*?:[^\s][^:]*?)\s*:(\s+|$)/.exec(t);
      if (!m) return null;
      if (/^[\[{]/.test(m[1])) return null;
      return { key: m[1], rest: t.slice(m[0].length).trim() };
    }

    function block(minIndent) {
      var l = cur();
      if (!l || l.indent < minIndent) return null;
      if (isSeq(l)) return seq(l.indent);
      if (/^[\[{]/.test(l.text)) {
        // A flow value on lines of its own, until it closes.
        var flow = l.text;
        i += 1;
        while (unclosed(flow) && i < lines.length) { flow += " " + lines[i].raw.trim(); i += 1; }
        return inlineValue(flow, l.no);
      }
      if (entry(l.text)) return map(l.indent);
      // A lone scalar (with its folded continuation lines).
      i += 1;
      var parts = [l.text];
      while (cur() && cur().indent >= minIndent && !isSeq(cur()) && !entry(cur().text)) { parts.push(cur().text); i += 1; }
      return inlineValue(parts.join(" "), l.no);
    }

    /// A | or > scalar: every following line indented past `indent`.
    function blockScalar(head, indent) {
      var folded = head[0] === ">";
      var keep = head.indexOf("+") >= 0, strip = head.indexOf("-") >= 0;
      var out = [], pad = -1;
      // `|2`: the content is indented exactly that much past its key.
      var given = /[1-9]/.exec(head);
      if (given) pad = indent + Number(given[0]);
      i += 1;
      while (i < lines.length) {
        var l = lines[i];
        var lead = l.raw.length - l.raw.replace(/^ +/, "").length;
        if (l.raw.trim() === "") { out.push(pad >= 0 && l.raw.length > pad ? l.raw.slice(pad) : ""); i += 1; continue; }
        if (lead <= indent) break;
        if (pad < 0) pad = lead;
        out.push(l.raw.slice(Math.min(pad, lead)));
        i += 1;
      }
      while (!keep && out.length && out[out.length - 1] === "") out.pop();
      var s = folded ? out.join("\n").replace(/([^\n])\n(?=[^\n ])/g, "$1 ") : out.join("\n");
      return strip || !s ? s : s + "\n";
    }

    /// The value after `key:` on a line indented `indent`.
    function valueAfter(rest, indent, no) {
      if (rest === "") {
        i += 1;
        var next = cur();
        if (!next) return null;
        if (next.indent > indent) return block(next.indent);
        if (next.indent === indent && isSeq(next)) return seq(indent);
        return null;
      }
      if (/^[|>][-+0-9]*$/.test(rest)) return blockScalar(rest, indent);
      if (/^<<$/.test(rest)) throw yamlRefusal(no, "this YAML uses merges (<<)");
      // A flow or quoted value may run onto the next lines.
      var text = rest;
      i += 1;
      if (/^["'\[{]/.test(text)) {
        while (unclosed(text) && i < lines.length) {
          var raw2 = lines[i].raw.trim();
          text += (/^["']/.test(rest) ? " " : "") + raw2;
          i += 1;
        }
        return inlineValue(text, no);
      }
      // A plain value folds the more-indented lines after it.
      while (cur() && cur().indent > indent && !isSeq(cur())) { text += " " + cur().text; i += 1; }
      return inlineValue(text, no);
    }

    function map(indent) {
      var out = {};
      for (;;) {
        var l = cur();
        if (!l || l.indent !== indent || isSeq(l)) break;
        var e = entry(l.text);
        if (!e) throw yamlRefusal(l.no, "a line is neither a key: value nor a - item");
        if (e.key === "<<") throw yamlRefusal(l.no, "this YAML uses merges (<<)");
        if (/^[&*!]/.test(e.rest)) throw yamlRefusal(l.no, "this YAML uses anchors, aliases or tags (& * !)");
        out[e.key] = valueAfter(e.rest, indent, l.no);
      }
      return out;
    }

    function seq(indent) {
      var out = [];
      for (;;) {
        var l = cur();
        if (!l || l.indent !== indent || !isSeq(l)) break;
        var rest = l.text === "-" ? "" : l.text.slice(2).trim();
        if (rest === "") {
          i += 1;
          var next = cur();
          out.push(next && next.indent > indent ? block(next.indent) : null);
        } else if (rest === "-" || rest.indexOf("- ") === 0) {
          // `- - item`: a list inside the list, indented where it starts.
          var inner = indent + (l.text.length - rest.length);
          lines[i] = { raw: new Array(inner + 1).join(" ") + rest, indent: inner, text: rest, no: l.no };
          out.push(seq(inner));
        } else if (!/^[\[{]/.test(rest) && entry(rest)) {
          // `- key: value` begins a mapping indented where its key is.
          var at = indent + (l.text.length - rest.length);
          lines[i] = { raw: new Array(at + 1).join(" ") + rest, indent: at, text: rest, no: l.no };
          out.push(map(at));
        } else if (/^[|>][-+0-9]*$/.test(rest)) {
          out.push(blockScalar(rest, indent));
        } else {
          out.push(valueAfter(rest, indent, l.no));
        }
      }
      return out;
    }

    var value = block(0);
    skip();
    if (i < lines.length) throw yamlRefusal(lines[i].no, "a line is indented less than the one before it expects");
    return value;
  }

  /// A description, as JSON or YAML text.
  function parse(text) {
    var t = String(text || "");
    if (!t.trim()) return problem("Nothing to read", "Paste the description, or drop its file here.");
    if (t.length > MAX_SPEC) return problem("That description is too big", "Krate reads descriptions up to 16 MB.");
    var s = t.replace(/^﻿/, "").trim();
    if (/^<(!doctype|html|\?xml)/i.test(s)) {
      return problem("That is a web page, not the description", "Open the description's own address (it ends in .json or .yaml), copy all of it, and paste it here.");
    }
    if (s[0] === "{") {
      try { return { spec: JSON.parse(s) }; } catch (e) {
        return problem("That JSON did not read", "Copy the whole description again, from the first { to the last }.");
      }
    }
    try {
      return { spec: parseYaml(s) };
    } catch (e) {
      if (e.yaml) return problem("That YAML did not read", "Krate read it up to where " + e.message + ". Paste the JSON version of the description instead: most API sites offer both.");
      throw e;
    }
  }

  // --------------------------------------------------- the description

  function isObj(v) { return v && typeof v === "object" && !Array.isArray(v); }

  /// A local $ref (#/components/parameters/x), followed; anything else as is.
  function deref(spec, v, depth) {
    depth = depth || 0;
    if (!isObj(v) || typeof v.$ref !== "string" || depth > 8) return v;
    if (v.$ref.indexOf("#/") !== 0) return v;
    var at = spec;
    v.$ref.slice(2).split("/").forEach(function (p) {
      p = p.replace(/~1/g, "/").replace(/~0/g, "~");
      at = isObj(at) ? at[p] : undefined;
    });
    return at === undefined ? v : deref(spec, at, depth + 1);
  }

  /// Every operation: {method, path, id, summary, query, body}.
  function operations(spec) {
    var out = [];
    var paths = isObj(spec.paths) ? spec.paths : {};
    Object.keys(paths).forEach(function (path) {
      if (path[0] !== "/") return;
      var item = deref(spec, paths[path]);
      if (!isObj(item)) return;
      var shared = Array.isArray(item.parameters) ? item.parameters : [];
      VERBS.forEach(function (verb) {
        var op = item[verb];
        if (!isObj(op)) return;
        var query = [], body = isObj(op.requestBody);
        shared.concat(Array.isArray(op.parameters) ? op.parameters : []).forEach(function (p) {
          p = deref(spec, p);
          if (!isObj(p) || typeof p.name !== "string") return;
          if (p.in === "body" || p.in === "formData") { body = true; return; }
          if (p.in === "query" || p.in === "header") {
            var name = clip(p.in === "header" ? "header " + p.name : p.name, 60);
            if (query.indexOf(name) < 0 && query.length < 12) query.push(name);
          }
        });
        var words = op.summary || (typeof op.description === "string" ? op.description.split(/(?<=[.!?])\s/)[0] : "");
        out.push({
          method: verb.toUpperCase(),
          path: path.slice(0, 200),
          id: clip(op.operationId || "", 80),
          summary: clip(words, 140),
          query: query,
          body: body,
        });
      });
    });
    return out;
  }

  /// The part of a path before its first {parameter}, whole segments.
  function staticPrefix(path) {
    var keep = [];
    var segs = String(path).split("/").filter(Boolean);
    for (var k = 0; k < segs.length; k += 1) {
      if (/[{}]/.test(segs[k])) break;
      keep.push(segs[k]);
    }
    return "/" + keep.join("/");
  }

  /// The shortest prefixes that cover these operations' paths, for these
  /// methods. Exact unless a description has more than MAX_PATHS separate
  /// prefixes: then they are cut back a segment at a time, and `widened`
  /// says the allow-list now also covers paths the description does not
  /// name (still only under the address).
  function pathsFor(ops, methods) {
    var want = (methods || []).map(function (m) { return String(m).toUpperCase(); });
    var set = [];
    var mine = (ops || []).filter(function (o) { return want.indexOf(o.method) >= 0; });
    // An operation on the address itself (GET /, an API's index) would
    // allow everything under it as a prefix, so it is left to the admin
    // unless it is all there is.
    var rooted = mine.filter(function (o) { return o.path !== "/"; });
    (rooted.length ? rooted : mine).forEach(function (o) {
      var p = staticPrefix(o.path);
      if (!GOOD_PATH.test(p) || /\/\.\.?(\/|$)/.test(p)) return;
      if (set.indexOf(p) < 0) set.push(p);
    });
    var collapse = function (list) {
      list = list.filter(function (p, k) { return list.indexOf(p) === k; }).sort();
      return list.filter(function (p) {
        return !list.some(function (q) { return q !== p && (q === "/" || p.indexOf(q + "/") === 0); });
      });
    };
    var paths = collapse(set);
    var widened = false;
    var depth = function (p) { return p.split("/").filter(Boolean).length; };
    // Too many: the deepest are cut back a segment at a time, so the
    // shallow ones stay exact as long as they can.
    while (paths.length > MAX_PATHS) {
      widened = true;
      var deepest = Math.max.apply(null, paths.map(depth));
      paths = collapse(paths.map(function (p) { return depth(p) === deepest ? "/" + p.split("/").filter(Boolean).slice(0, deepest - 1).join("/") : p; }));
    }
    return { paths: paths, widened: widened };
  }

  /// The address the description names, from servers (OpenAPI 3) or
  /// schemes, host and basePath (Swagger 2), resolved against where it was
  /// read from. Answers {base} (null when it cannot know the site),
  /// {needs: [...]} for variables the admin fills in, or {problem}.
  function baseOf(spec, url) {
    var candidates = [];
    if (spec.openapi) {
      var servers = Array.isArray(spec.servers) && spec.servers.length ? spec.servers : [{ url: "/" }];
      servers.forEach(function (s) {
        if (!isObj(s) || typeof s.url !== "string") return;
        var vars = isObj(s.variables) ? s.variables : {};
        candidates.push(s.url.replace(/\{([^}]+)\}/g, function (all, name) {
          var v = vars[name];
          return isObj(v) && v.default !== undefined && v.default !== "" ? String(v.default) : "<" + name + ">";
        }));
      });
    } else {
      var schemes = Array.isArray(spec.schemes) ? spec.schemes.map(String) : [];
      var scheme = schemes.length ? (schemes.indexOf("https") >= 0 ? "https" : schemes[0]) : null;
      var host = typeof spec.host === "string" && spec.host ? spec.host : null;
      var basePath = typeof spec.basePath === "string" ? spec.basePath : "";
      if (host) candidates.push((scheme || "https") + "://" + host + basePath);
      else candidates.push((scheme ? scheme + ":" : "") + (basePath || "/"));
    }
    var resolved = candidates.map(function (c) {
      if (/^[a-z][a-z0-9+.-]*:\/\//i.test(c)) return c;
      if (!url) return { relative: c };
      try { return new URL(c, url).href; } catch (e) { return { relative: c }; }
    });
    var absolute = resolved.filter(function (r) { return typeof r === "string"; });
    if (!absolute.length) {
      return { base: null, relative: resolved[0] && resolved[0].relative };
    }
    var https = absolute.filter(function (r) { return /^https:\/\//i.test(r); });
    if (!https.length) {
      return problem("This API is not on https", "Its description gives the address " + absolute[0] + ". Krate sends credentials only over https, so they are never sent in the clear.");
    }
    var b = https[0].replace(/[?#].*$/, "").replace(/\/+$/, "");
    // Keep the host as typed: a placeholder like <instance> is not a URL.
    var m = /^https:\/\/([^/]+)(\/.*)?$/i.exec(b);
    var shaped = m ? "https://" + m[1].toLowerCase() + (m[2] || "") : b;
    var needs = [];
    shaped.replace(/<([^>]+)>/g, function (all, name) { if (needs.indexOf(name) < 0) needs.push(name); return all; });
    return { base: shaped, needs: needs };
  }

  /// The credential the description asks for, in the form's terms:
  /// {kind: "header", header, prefix?} or {kind: "basic"}, with the words
  /// for the admin; null when it names none Krate can send.
  function authOf(spec) {
    var schemes = spec.openapi
      ? isObj(spec.components) && isObj(spec.components.securitySchemes) ? spec.components.securitySchemes : {}
      : isObj(spec.securityDefinitions) ? spec.securityDefinitions : {};
    var names = Object.keys(schemes);
    if (!names.length) return { auth: null, note: null };
    // The schemes the API asks for first, then the rest.
    var used = [];
    (Array.isArray(spec.security) ? spec.security : []).forEach(function (req) {
      if (isObj(req)) Object.keys(req).forEach(function (k) { if (used.indexOf(k) < 0 && schemes[k]) used.push(k); });
    });
    names.forEach(function (k) { if (used.indexOf(k) < 0) used.push(k); });
    // A key, a token or a password first: a connection keeps one fixed
    // credential, and an OAuth sign-in's tokens run out.
    var lasting = function (k) { var t = String((deref(spec, schemes[k]) || {}).type || "").toLowerCase(); return t !== "oauth2" && t !== "openidconnect"; };
    used = used.filter(lasting).concat(used.filter(function (k) { return !lasting(k); }));
    var skipped = null;
    for (var k = 0; k < used.length; k += 1) {
      var s = deref(spec, schemes[used[k]]);
      if (!isObj(s)) continue;
      var type = String(s.type || "").toLowerCase();
      var scheme = String(s.scheme || "").toLowerCase();
      if (type === "apikey") {
        if (s.in === "header" && typeof s.name === "string" && /^[A-Za-z0-9!#$%&'*+.^_`|~-]{1,64}$/.test(s.name)) {
          return { auth: { kind: "header", header: s.name }, note: "It takes a key in the " + s.name + " header." };
        }
        skipped = skipped || "It takes its key in the " + (s.in === "query" ? "address (?" + (s.name || "key") + "=...)" : s.in || "request") + ", and Krate adds credentials as a header only. Check whether the API also takes the key as a header.";
        continue;
      }
      if (type === "basic" || (type === "http" && scheme === "basic")) {
        return { auth: { kind: "basic" }, note: "It takes a user and password." };
      }
      if (type === "http" && scheme === "bearer") {
        return { auth: { kind: "header", header: "Authorization", prefix: "Bearer " }, note: "It takes a token, sent as Bearer." };
      }
      if (type === "oauth2" || type === "openidconnect") {
        return {
          auth: { kind: "header", header: "Authorization", prefix: "Bearer " },
          note: "It signs in with OAuth. Paste an access token that lasts (a service account's or an app's), sent as Bearer: Krate does not refresh tokens.",
        };
      }
      if (type === "http") {
        return { auth: { kind: "header", header: "Authorization" }, note: "It takes the Authorization header (" + (s.scheme || "http") + ")." };
      }
    }
    return { auth: null, note: skipped };
  }

  /// The compact summary the hub keeps and Studio's AI reads. The
  /// operations the connection allows (`allow`, when given) come first, so
  /// a big API's cap leaves out the ones apps cannot call anyway.
  function summary(spec, ops, url, allow, notes) {
    if (allow) {
      var conn = { allow: allow };
      ops = ops.filter(function (o) { return covered(conn, o); }).concat(ops.filter(function (o) { return !covered(conn, o); }));
    }
    var info = isObj(spec.info) ? spec.info : {};
    var source = url && /^https:\/\//i.test(url) ? String(url).slice(0, 300) : "pasted";
    var api = { title: clip(info.title || "API", 120), version: clip(info.version || "", 40), source: source };
    if (notes) api.notes = clip(notes, 600);
    api.operations = [];
    api.more = 0;
    var size = JSON.stringify(api).length;
    for (var k = 0; k < ops.length; k += 1) {
      var o = ops[k];
      var item = { method: o.method, path: o.path };
      if (o.id) item.id = o.id;
      if (o.summary) item.summary = o.summary;
      if (o.query.length) item.query = o.query;
      if (o.body) item.body = true;
      var add = JSON.stringify(item).length + 1;
      // Measured in UTF-8 bytes, as the hub measures it.
      var bytes = typeof TextEncoder !== "undefined" ? new TextEncoder().encode(JSON.stringify(item)).length + 1 : add;
      if (size + bytes > MAX_API - 16) { api.more = ops.length - k; break; }
      api.operations.push(item);
      size += bytes;
    }
    return api;
  }

  /// Everything the connection form is filled with.
  function fromSpec(spec, opt) {
    opt = opt || {};
    if (!isObj(spec)) return problem("That is not an API description", "Paste an OpenAPI or Swagger description: it starts with openapi: 3 or swagger: \"2.0\".");
    var v3 = typeof spec.openapi === "string" && /^3\./.test(spec.openapi);
    var v2 = String(spec.swagger) === "2.0";
    if (!v3 && !v2) {
      return problem("That is not an OpenAPI 3 or Swagger 2 description", spec.openapi || spec.swagger
        ? "It says version " + String(spec.openapi || spec.swagger).slice(0, 12) + ". Krate reads OpenAPI 3 and Swagger 2.0."
        : "It has no openapi: or swagger: line at the top. Check you copied the description itself, not a page about it.");
    }
    var ops = operations(spec);
    if (!ops.length) return problem("This description lists no operations", "It has no paths with a get, post or other method under them, so there is nothing to allow.");
    var where = baseOf(spec, opt.url);
    if (where.problem) return where;
    var methods = FORM_METHODS.filter(function (m) { return ops.some(function (o) { return o.method === m; }); });
    var ticked = methods.indexOf("GET") >= 0 ? ["GET"] : [];
    var allow = pathsFor(ops, ticked.length ? ticked : methods);
    var a = authOf(spec);
    var notes = [];
    if (where.base === null) notes.push("The description gives its address as " + (where.relative || "/") + " without the site. Type the full address.");
    if ((where.needs || []).length) notes.push("Replace " + where.needs.map(function (n) { return "<" + n + ">"; }).join(" and ") + " in the address with yours.");
    if (a.note) notes.push(a.note);
    var others = methods.filter(function (m) { return ticked.indexOf(m) < 0; });
    if (!ticked.length) notes.push("This API has no GET operations. Tick what apps must do: " + others.join(", ") + ".");
    else if (others.length) notes.push("It also has " + others.join(", ") + ". Tick them only if apps must change things.");
    if (allow.widened) notes.push("It has more than " + MAX_PATHS + " separate paths, so some prefixes are shorter and also cover paths it does not name.");
    var api = summary(spec, ops, opt.url, { methods: ticked.length ? ticked : methods, paths: allow.paths });
    return {
      spec: spec,
      url: opt.url || null,
      base: where.base,
      needs: where.needs || [],
      title: api.title,
      methods: methods,
      ticked: ticked,
      ops: ops,
      paths: allow.paths,
      widened: allow.widened,
      auth: a.auth,
      api: api,
      notes: notes,
    };
  }

  // ------------------------------------------------- what the AI reads

  function covered(conn, op) {
    var a = conn.allow || {};
    var methods = a.methods || ["GET"];
    var paths = a.paths || ["/"];
    if (methods.indexOf(op.method) < 0) return false;
    return paths.some(function (p) { return p === "/" || op.path === p || op.path.indexOf(p + "/") === 0; });
  }

  /// The Markdown a build is handed about one connection: how to declare
  /// it, its address, what it allows, and from its description the
  /// operations it allows (the rest are counted, never listed, so the AI
  /// does not write calls that would be refused).
  function contextText(conn, orgName) {
    var a = conn.allow || {};
    var methods = a.methods || ["GET"];
    var paths = a.paths || ["/"];
    var org = orgName || "the organisation";
    var lines = [
      "# Company connection: " + conn.name + " (" + org + ")",
      "",
      "The person asked for an app that uses " + org + "'s `" + conn.name + "` connection. Declare it in manifest.toml, and nothing else for this API (no net.connect, no store.secret, no token box):",
      "",
      "```toml",
      "[[capabilities]]",
      "cap = \"net.connection:" + conn.name + "\"",
      "rationale = \"Read from " + (conn.api && conn.api.title ? conn.api.title.replace(/"/g, "'") : conn.name) + "\"",
      "```",
      "",
      "Send ordinary net requests to " + conn.base + " followed by the path, WITHOUT any Authorization or key header: Krate adds " + org + "'s credential on the way out and the app never sees it.",
      "",
      "Allowed: " + methods.join(", ") + " on " + (paths.length === 1 && paths[0] === "/" ? "every path under the address" : paths.join(", ")) + ". Anything else comes back as net-error::other with a sentence saying why; show that sentence as it is.",
    ];
    var api = conn.api;
    if (api && api.notes) lines.push("", "About this API: " + api.notes);
    if (api && Array.isArray(api.operations) && api.operations.length) {
      var ok = api.operations.filter(function (o) { return covered(conn, o); });
      var not = api.operations.length - ok.length;
      lines.push("", "## " + (api.title || "The API") + (api.version ? " " + api.version : "") + ", from its OpenAPI description", "");
      if (!ok.length) lines.push("None of the operations in its description are allowed by this connection. Tell the person an admin of " + org + " can allow more in the organisation console.");
      ok.forEach(function (o) {
        var bits = ["- `" + o.method + " " + o.path + "`"];
        if (o.id) bits.push("(" + o.id + ")");
        if (o.summary) bits.push("-- " + o.summary);
        if (o.query && o.query.length) bits.push("Query: " + o.query.join(", ") + ".");
        if (o.body) bits.push("Takes a JSON body.");
        lines.push(bits.join(" "));
      });
      var left = not + (api.more || 0);
      if (left) lines.push("", left + " more operation" + (left === 1 ? " is" : "s are") + " in the API but not allowed here" + (api.more ? " or not listed" : "") + ": do not call them.");
      lines.push("", "Paths are relative to " + conn.base + ". Fill {parameters} in the path; read the JSON the API answers defensively, since its description is a summary.");
    }
    return lines.join("\n") + "\n";
  }

  // ------------------------------------------------------------ presets
  //
  // The systems buyers name first. Each one is a starting point the admin
  // can still edit: the address (with <placeholders> they fill), the kind
  // of credential, read-only defaults, and the vendor's own page on making
  // the credential. Checked against each vendor's documentation, 2026-10.

  var BEARER = function (what) { return { kind: "header", header: "Authorization", prefix: "Bearer ", secret: what }; };
  /// A token that expires within the hour: said where the admin decides.
  var SHORT = "Its access tokens last about an hour, and a connection keeps one fixed credential, so the token must be renewed for Krate when it runs out.";
  var PRESETS = [
    {
      id: "graph", label: "Microsoft Graph", what: "SharePoint, Outlook, Teams", name: "microsoft-graph",
      base: "https://graph.microsoft.com/v1.0", auth: BEARER("an access token"),
      methods: ["GET"], paths: ["/sites", "/users", "/groups"],
      docs: "https://learn.microsoft.com/en-us/graph/auth-register-app-v2",
      note: "Register an app in Microsoft Entra and give it read permissions. " + SHORT,
    },
    {
      id: "google", label: "Google Workspace", what: "Drive", name: "google-drive",
      base: "https://www.googleapis.com/drive/v3", auth: BEARER("an access token"),
      methods: ["GET"], paths: ["/files", "/drives", "/about"],
      docs: "https://developers.google.com/workspace/guides/create-credentials",
      note: "For Gmail use https://gmail.googleapis.com/gmail/v1, for the user directory https://admin.googleapis.com/admin/directory/v1. Google's access tokens expire, and a connection keeps one fixed credential, so the token must be renewed for Krate.",
    },
    {
      id: "salesforce", label: "Salesforce", what: "REST API", name: "salesforce",
      base: "https://<my-domain>.my.salesforce.com/services/data/v65.0", auth: BEARER("an access token"),
      methods: ["GET"], paths: ["/query", "/sobjects", "/limits"],
      docs: "https://help.salesforce.com/s/articleView?id=sf.meta_configure_client_credentials_flow_for_external_client_apps.htm&language=en_US&type=5",
      note: "Use your My Domain, and the API version your org lists at /services/data/. The token comes from an External Client App's run-as user; it expires, so it must be renewed for Krate.",
    },
    {
      id: "servicenow", label: "ServiceNow", what: "Table API", name: "servicenow",
      base: "https://<instance>.service-now.com/api/now", auth: { kind: "basic", user: "An integration user", secret: "Its password" },
      methods: ["GET"], paths: ["/table/incident", "/table/sys_user"],
      docs: "https://www.servicenow.com/docs/bundle/zurich-api-reference/page/integrate/inbound-rest/concept/c_TableAPI.html",
      note: "A ServiceNow user with read roles on the tables your apps read.",
      ai: "Table API rows: GET /table/<table>?sysparm_limit=20. Ask for JSON with Accept: application/json.",
    },
    {
      id: "jira", label: "Jira", what: "Cloud REST API", name: "jira",
      base: "https://<site>.atlassian.net/rest/api/3", auth: { kind: "basic", user: "Your Atlassian email", secret: "An API token" },
      methods: ["GET"], paths: ["/search/jql", "/project", "/myself"],
      docs: "https://support.atlassian.com/atlassian-account/docs/manage-api-tokens-for-your-atlassian-account/",
      note: "Use a token made without scopes with this address. A scoped token works only through https://api.atlassian.com/ex/jira/<cloud-id>/rest/api/3.",
      ai: "Search issues with GET /search/jql?jql=...&fields=summary,status (the older /search is being removed).",
    },
    {
      id: "slack", label: "Slack", what: "Web API", name: "slack",
      base: "https://slack.com/api", auth: BEARER("a bot token (xoxb-...)"),
      methods: ["GET"], paths: ["/conversations.list", "/conversations.history", "/users.list"],
      docs: "https://docs.slack.dev/app-management/quickstart-app-settings",
      note: "Make a Slack app, give its bot read scopes, install it, and paste its bot token.",
      ai: "Slack methods are /<family>.<method> with query arguments, such as GET /conversations.list?limit=100. Every answer has ok: true or ok: false with an error.",
    },
    {
      id: "hubspot", label: "HubSpot", what: "CRM", name: "hubspot",
      base: "https://api.hubapi.com", auth: BEARER("a service key"),
      methods: ["GET"], paths: ["/crm/v3/objects/contacts", "/crm/v3/objects/companies", "/crm/v3/objects/deals"],
      docs: "https://developers.hubspot.com/docs/apps/developer-platform/build-apps/authentication/account-service-keys",
      note: "In HubSpot: Development, Keys, Service keys, with read scopes only.",
    },
    {
      id: "stripe", label: "Stripe", what: "Payments", name: "stripe",
      base: "https://api.stripe.com/v1", auth: BEARER("a restricted key (rk_...)"),
      methods: ["GET"], paths: ["/customers", "/charges", "/balance"],
      docs: "https://docs.stripe.com/keys/restricted-api-keys",
      note: "Use a restricted key with Read access only, never the secret key.",
    },
    {
      id: "snowflake", label: "Snowflake", what: "SQL API", name: "snowflake",
      base: "https://<account>.snowflakecomputing.com/api/v2", auth: BEARER("a programmatic access token"),
      methods: ["GET", "POST"], paths: ["/statements"],
      docs: "https://docs.snowflake.com/en/user-guide/programmatic-access-tokens",
      note: "Every statement is sent with POST, even a SELECT, so POST is ticked: give the token's user a role that can only read.",
      ai: "Run SQL with POST /statements and a JSON body {\"statement\": \"select ...\", \"timeout\": 60}, Content-Type and Accept application/json. A long query answers 202 with a statementHandle: poll GET /statements/<handle>.",
    },
    {
      id: "github", label: "GitHub", what: "REST API", name: "github",
      base: "https://api.github.com", auth: BEARER("a fine-grained token"),
      methods: ["GET"], paths: ["/repos", "/orgs", "/user"],
      docs: "https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/managing-your-personal-access-tokens",
      note: "A fine-grained token with read access to the repositories apps need.",
      ai: "Every request must send a User-Agent header, Accept: application/vnd.github+json and X-GitHub-Api-Version: 2026-03-10.",
    },
    {
      id: "zendesk", label: "Zendesk", what: "Support API", name: "zendesk",
      base: "https://<subdomain>.zendesk.com/api/v2", auth: { kind: "basic", user: "Your email, then /token (you@acme.com/token)", secret: "An API token" },
      methods: ["GET"], paths: ["/tickets", "/users", "/search"],
      docs: "https://support.zendesk.com/hc/en-us/articles/4408889192858-Managing-access-to-the-Zendesk-API",
      note: "Zendesk marks API tokens as deprecated in favour of OAuth access tokens. An OAuth token works too: choose A header and paste Bearer and the token.",
      ai: "Resources end in .json, such as GET /tickets.json and GET /search.json?query=...",
    },
    {
      id: "n8n", label: "n8n webhook", what: "Automation", name: "n8n-webhook",
      base: "", basePlaceholder: "https://n8n.acme.com/webhook/...", auth: { kind: "header", header: "X-Krate-Key", secret: "The header auth value" },
      methods: ["POST"], paths: ["/"],
      docs: "https://docs.n8n.io/integrations/builtin/core-nodes/n8n-nodes-base.webhook/",
      note: "Set the Webhook node's authentication to Header Auth, with the same header name and value as here, and paste its production URL as the address.",
      ai: "Send the event as a JSON POST to the address itself (path /).",
    },
    {
      id: "make", label: "Make webhook", what: "Automation", name: "make-webhook",
      base: "", basePlaceholder: "The webhook address Make shows", auth: { kind: "header", header: "x-make-apikey", secret: "The webhook's API key" },
      methods: ["POST"], paths: ["/"],
      docs: "https://apps.make.com/gateway",
      note: "Add an API key to the custom webhook in Make, so its address alone is not enough to call it.",
      ai: "Send the event as a JSON POST to the address itself (path /).",
    },
    {
      id: "rest", label: "Other REST API", what: "", name: "",
      base: "", basePlaceholder: "https://api.acme.com/finance", auth: { kind: "header", header: "Authorization" },
      methods: ["GET"], paths: ["/"],
      docs: "", note: "",
    },
  ];

  root.KrateConnect = {
    MAX_API: MAX_API,
    MAX_PATHS: MAX_PATHS,
    FORM_METHODS: FORM_METHODS,
    parse: parse,
    parseYaml: parseYaml,
    fromSpec: fromSpec,
    pathsFor: pathsFor,
    staticPrefix: staticPrefix,
    summary: summary,
    operations: operations,
    contextText: contextText,
    PRESETS: PRESETS,
  };
})(typeof window !== "undefined" ? window : globalThis);
