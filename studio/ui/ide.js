/* Krate Studio's IDE: a Krate project's files, the code, a terminal and a
 * live picture of the app, with your AI one keystroke away.
 *
 * Desktop only. Every action calls a command in studio/src/ide.rs, which
 * drives the same `krate` engine the rest of Studio uses: check-app builds
 * the project and paints its first frame (the preview), pack makes the
 * .krate, revise lets the AI change the source. In a browser there is no
 * project folder to open, so the IDE's two doors (the sidebar row and the
 * Home tab) are taken away and nothing here runs.
 *
 * Loaded after app.js and before redesign.js. Everything sits inside one
 * function: app.js and bridge.js share one global scope in a tab, and a
 * clash blanks the page (scope-test.mjs). It reads app.js's `tauri`,
 * `invoke`, `state` and `showView`, and writes none of them.
 */
(function () {
  "use strict";
  const $ = (id) => document.getElementById(id);
  const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));
  const desktop = (() => { try { return !!tauri; } catch (e) { return false; } })();

  if (!desktop) {
    // No IDE in a browser: no door to it.
    ["sideIde", "homeIdeTab"].forEach((id) => { const el = $(id); if (el) el.remove(); });
    return;
  }

  const isMac = /mac/i.test(navigator.platform || navigator.userAgent);
  const MOD = isMac ? "⌘" : "Ctrl ";
  const call = (cmd, args) => invoke(cmd, args);
  const toast = (t) => { if (window.krToast) window.krToast(t); };
  const short = (p) => String(p || "").replace(/^\/Users\/[^/]+\//, "~/").replace(/^\/home\/[^/]+\//, "~/").replace(/^C:\\Users\\[^\\]+\\/i, "~\\");
  const kb = (n) => n == null ? "" : n < 1024 ? `${n} B` : n < 1048576 ? `${Math.round(n / 1024)} KB` : `${(n / 1048576).toFixed(1)} MB`;
  const ago = (secs) => {
    if (!secs) return "";
    const d = Math.floor(Date.now() / 1000) - secs;
    if (d < 90) return "just now";
    if (d < 3600) return `${Math.floor(d / 60)} min ago`;
    if (d < 86400) return `${Math.floor(d / 3600)} h ago`;
    const days = Math.round(d / 86400);
    return days <= 1 ? "yesterday" : `${days} days ago`;
  };
  const agentName = () => { try { return state.agent || "claude"; } catch (e) { return "claude"; } };
  const agentWords = () => { try { return agentLabel(); } catch (e) { return "Your AI"; } };

  /* ---- pictures of projects, kept small, for the Home tiles ------------ */
  const SHOTS = "krate-ide-shots";
  const shots = () => { try { return JSON.parse(localStorage.getItem(SHOTS) || "{}"); } catch (e) { return {}; } };
  function keepShot(path, dataUrl) {
    const img = new Image();
    img.onload = () => {
      try {
        const w = 320, h = Math.round(img.height * (w / img.width));
        const c = document.createElement("canvas"); c.width = w; c.height = h;
        c.getContext("2d").drawImage(img, 0, 0, w, h);
        const all = shots(); all[path] = { at: Date.now(), src: c.toDataURL("image/jpeg", 0.8) };
        const keep = Object.entries(all).sort((a, b) => b[1].at - a[1].at).slice(0, 12);
        localStorage.setItem(SHOTS, JSON.stringify(Object.fromEntries(keep)));
      } catch (e) { /* a full or blocked store: the tile just has no picture */ }
    };
    img.src = dataUrl;
  }

  /* ---- the editor's engine: CodeMirror, loaded the first time it is needed */
  // vendor/codemirror.js is half a megabyte, so it is fetched when a project
  // first opens, never on the web (there is no IDE there) and never at launch.
  let CM = null, cmLoading = null;
  function loadCM() {
    if (window.KrateCM) return Promise.resolve((CM = window.KrateCM));
    if (!cmLoading) cmLoading = new Promise((res, rej) => {
      const sc = document.createElement("script");
      sc.src = "vendor/codemirror.js";
      sc.onload = () => (window.KrateCM ? res((CM = window.KrateCM)) : rej(new Error("the editor did not load")));
      sc.onerror = () => rej(new Error("the editor could not load"));
      document.head.appendChild(sc);
    });
    // A failure is not kept: the next file opened tries again.
    cmLoading.catch(() => { cmLoading = null; });
    return cmLoading;
  }
  // The same colours as the Code pane, as classes, so light and dark follow.
  let hlStyle = null, edTheme = null, setAdds = null, addsField = null;
  function cmParts() {
    if (hlStyle) return;
    const t = CM.tags;
    hlStyle = CM.HighlightStyle.define([
      { tag: [t.keyword, t.controlKeyword, t.definitionKeyword, t.modifier, t.operatorKeyword, t.self], class: "tk-k" },
      { tag: [t.string, t.character, t.special(t.string)], class: "tk-s" },
      { tag: [t.comment, t.lineComment, t.blockComment, t.docComment], class: "tk-c" },
      { tag: [t.number, t.bool, t.atom, t.null], class: "tk-n" },
      { tag: [t.typeName, t.className, t.namespace, t.heading, t.standard(t.typeName)], class: "tk-t" },
      { tag: [t.function(t.variableName), t.function(t.propertyName), t.macroName], class: "tk-f" },
      { tag: [t.meta, t.attributeName, t.annotation, t.processingInstruction], class: "tk-a" },
    ]);
    edTheme = CM.EditorView.theme({
      "&": { height: "100%", fontSize: "12.5px", color: "var(--ink)", backgroundColor: "transparent" },
      "&.cm-focused": { outline: "none" },
      ".cm-scroller": { fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace", lineHeight: "1.75" },
      ".cm-content": { padding: "14px 0 120px", caretColor: "var(--ink)" },
      ".cm-gutters": { backgroundColor: "transparent", border: "none", color: "var(--ink-4)" },
      ".cm-lineNumbers .cm-gutterElement": { padding: "0 12px 0 16px" },
      ".cm-activeLine": { backgroundColor: "color-mix(in srgb, var(--ink) 3.5%, transparent)" },
      ".cm-activeLineGutter": { backgroundColor: "transparent", color: "var(--ink-2)" },
      ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--ink)" },
      "&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, .cm-selectionBackground, .cm-content ::selection": { backgroundColor: "color-mix(in srgb, var(--accent) 24%, transparent) !important" },
      ".cm-matchingBracket": { backgroundColor: "color-mix(in srgb, var(--accent) 18%, transparent)", outline: "none" },
      ".cm-selectionMatch": { backgroundColor: "color-mix(in srgb, var(--accent) 10%, transparent)" },
      ".cm-panels": { backgroundColor: "var(--surface)", color: "var(--ink)" },
      ".cm-panels.cm-panels-top": { borderBottom: "1px solid var(--line)" },
      ".cm-searchMatch": { backgroundColor: "color-mix(in srgb, var(--orange) 22%, transparent)" },
      ".cm-searchMatch-selected": { backgroundColor: "color-mix(in srgb, var(--orange) 45%, transparent)" },
      ".cm-tooltip": { backgroundColor: "var(--surface)", border: "none", borderRadius: "10px", boxShadow: "0 0 0 1px var(--hair), 0 12px 30px -12px rgba(0,0,0,.3)" },
      ".cm-tooltip-autocomplete > ul > li[aria-selected]": { backgroundColor: "var(--surface-2)", color: "var(--ink)" },
      ".cm-foldGutter .cm-gutterElement": { color: "var(--ink-4)" },
      ".cm-line.kr-add": { backgroundColor: "color-mix(in srgb, var(--green) 12%, transparent)" },
    });
    // The lines the AI just added, marked where they landed.
    setAdds = CM.StateEffect.define();
    addsField = CM.StateField.define({
      create: () => CM.Decoration.none,
      update(deco, tr) {
        for (const e of tr.effects) if (e.is(setAdds)) {
          const b = new CM.RangeSetBuilder();
          for (const n of [...e.value].sort((x, y) => x - y)) {
            if (n < tr.state.doc.lines) { const line = tr.state.doc.line(n + 1); b.add(line.from, line.from, CM.Decoration.line({ class: "kr-add" })); }
          }
          return b.finish();
        }
        return tr.docChanged ? CM.Decoration.none : deco;
      },
      provide: (f) => CM.EditorView.decorations.from(f),
    });
  }
  /* ---- Krate knowledge: the engine's own description of the API ---------- */
  // `krate sdk-reference` (via ide_sdk): every guest function, the
  // capability names, the std that is refused. Read once per Studio.
  let SDK = null;
  async function loadSdk() {
    // A failure is not kept: the next open asks the engine again.
    if (SDK && SDK.functions && SDK.functions.length) return SDK;
    try { SDK = await call("ide_sdk"); } catch (e) { SDK = null; return { functions: [], capabilities: [], granted: [], leaks: [] }; }
    return SDK;
  }
  const sdk = () => SDK || { functions: [], capabilities: [], granted: [], leaks: [] };
  const fnsFree = () => sdk().functions.filter((f) => !f.method);
  // Completion: `stdio::` lists what is in it; a word lists the modules and
  // calls it could start; inside a manifest's quotes, the capability names.
  // The file a view shows. Asked of the view, not of the tab in front: a
  // view keeps working on its file after a rename, and a background view's
  // linter runs too.
  function relOfView(view) {
    if (ide && view) for (const [r, d] of ide.docs) if (d.view === view) return r;
    return ide && ide.cur || "";
  }
  function krateComplete(ctx) {
    const rel = relOfView(ctx.view);
    if (/manifest\.toml$/.test(rel)) {
      const m = ctx.matchBefore(/"[a-z._:<>\-*\/]*/); if (!m) return null;
      const opts = [...sdk().capabilities, ...sdk().granted].map((c) => ({ label: c, type: "constant", detail: sdk().granted.includes(c) ? "every app has it" : "asks the person" }));
      return { from: m.from + 1, options: opts, validFor: /^[a-z._:<>\-*\/]*$/ };
    }
    if (!/\.rs$/.test(rel)) return null;
    const m = ctx.matchBefore(/[A-Za-z_][\w]*(::[A-Za-z_]*)*:?:?/);
    if (!m || (m.from === m.to && !ctx.explicit)) return null;
    const text = m.text.replace(/^krate::/, "");
    const at = text.lastIndexOf("::");
    if (at >= 0) {
      const prefix = text.slice(0, at), from = m.from + (m.text.length - text.length) + at + 2;
      const mods = new Set(), opts = [];
      for (const f of fnsFree()) {
        if (f.module === prefix || f.module.endsWith("::" + prefix)) opts.push({ label: f.name, type: "function", detail: `(${f.params}) -> ${f.returns}`, apply: f.name + "(" });
        const deeper = f.module.startsWith(prefix + "::") ? f.module.slice(prefix.length + 2).split("::")[0] : f.module.includes("::" + prefix + "::") ? f.module.split("::" + prefix + "::")[1].split("::")[0] : "";
        if (deeper) mods.add(deeper);
      }
      for (const d of mods) opts.push({ label: d, type: "namespace", apply: d + "::" });
      return opts.length ? { from, options: opts, validFor: /^\w*$/ } : null;
    }
    if (text.length < 2 && !ctx.explicit) return null;
    const roots = new Set(fnsFree().map((f) => f.module.split("::")[0]));
    const opts = [...roots].map((r) => ({ label: r, type: "namespace", detail: "Krate", apply: r + "::" }))
      .concat(fnsFree().map((f) => ({ label: `${f.module}::${f.name}`, type: "function", detail: `(${f.params}) -> ${f.returns}`, apply: `${f.module}::${f.name}(` })));
    return { from: m.from, options: opts, validFor: /^[\w:]*$/ };
  }
  // Hover: the signature of a Krate call under the pointer.
  function krateHover(view, pos) {
    if (!/\.rs$/.test(relOfView(view))) return null;
    const line = view.state.doc.lineAt(pos), off = pos - line.from, t = line.text;
    let a = off, z = off;
    while (a > 0 && /[\w:]/.test(t[a - 1])) a--;
    while (z < t.length && /\w/.test(t[z])) z++;
    const word = t.slice(a, z).replace(/^krate::/, ""); if (!word) return null;
    const parts = word.split("::"), name = parts.pop(), mod = parts.join("::");
    const hits = sdk().functions.filter((f) => f.name === name && (!mod || f.module === mod || f.module.endsWith("::" + mod) || f.receiver === mod)).slice(0, 3);
    if (!hits.length) return null;
    return { pos: line.from + a, end: line.from + z, above: true, create() {
      const dom = document.createElement("div"); dom.className = "kr-hov";
      dom.innerHTML = hits.map((f) => `<code>${esc(f.signature)}</code>`).join("") + '<small>Krate API</small>';
      return { dom };
    } };
  }
  // As you type: std that reaches the operating system (the app would be
  // refused at the import check), manifest capability names Krate does not
  // know, and what the last build said about this file.
  function krateLint(view) {
    const rel = relOfView(view); const out = [];
    const doc = view.state.doc;
    if (/\.rs$/.test(rel)) {
      const leaks = sdk().leaks || [];
      for (let n = 1; n <= doc.lines; n++) {
        const line = doc.line(n), code = line.text.replace(/\/\/.*$/, "").replace(/"(?:[^"\\]|\\.)*"/g, (m) => " ".repeat(m.length));
        for (const l of leaks) {
          const rx = new RegExp((l.pattern.endsWith("!") ? "(?<![\\w:])" : "\\b") + l.pattern.replace(/[.*+?^${}()|[\]\\]/g, "\\$&").replace(/!$/, "!"), "g");
          let m;
          while ((m = rx.exec(code))) out.push({ from: line.from + m.index, to: line.from + m.index + m[0].length, severity: "error", message: `${l.pattern} reaches ${l.reaches} through the operating system, so the app would be refused at the import check. Use Krate's own API instead (the Krate API tab lists it).` });
        }
      }
    } else if (/manifest\.toml$/.test(rel)) {
      const known = new Set([...sdk().capabilities, ...sdk().granted].map((c) => c.split(":")[0]));
      let inCaps = false;
      for (let n = 1; n <= doc.lines && known.size; n++) {
        const line = doc.line(n), t = line.text.replace(/#.*$/, "");
        const h = t.match(/^\s*\[([^\]]+)\]/); if (h) { inCaps = /capabilit/i.test(h[1]); continue; }
        if (!inCaps) continue;
        const rx = /"([^"]*)"/g; let m;
        while ((m = rx.exec(t))) {
          const base = m[1].split(":")[0];
          if (base && !known.has(base)) out.push({ from: line.from + m.index, to: line.from + m.index + m[0].length, severity: "error", message: `${base} is not a capability Krate knows, so the app would be refused when it is packed.` });
        }
      }
    }
    for (const d of (ide && ide.diags || []).filter((x) => x.file === rel)) {
      const ln = doc.line(Math.max(1, Math.min(d.line, doc.lines)));
      const from = Math.min(ln.from + Math.max(0, d.col - 1), ln.to);
      const word = doc.sliceString(from, ln.to).match(/^[\w:.]+/);
      out.push({ from, to: Math.min(ln.to, from + Math.max(1, word ? word[0].length : 1)), severity: d.sev === "warning" ? "warning" : "error", message: d.msg + (d.code ? ` (${d.code})` : "") });
    }
    return out;
  }

  function extensionsFor(rel) {
    const K = CM; cmParts();
    const lang = /\.rs$/.test(rel) ? K.rust() : /\.toml$/.test(rel) ? K.toml : [];
    return [
      K.lintGutter(), K.lineNumbers(), K.highlightActiveLineGutter(), K.highlightSpecialChars(), K.history(),
      K.foldGutter({ openText: "▾", closedText: "▸" }), K.drawSelection(), K.dropCursor(),
      K.EditorState.allowMultipleSelections.of(true), K.indentOnInput(), K.syntaxHighlighting(hlStyle),
      K.bracketMatching(), K.closeBrackets(), K.autocompletion({ override: [krateComplete], icons: false }), K.hoverTooltip(krateHover), K.linter(krateLint, { delay: 400 }), K.rectangularSelection(), K.crosshairCursor(),
      K.highlightActiveLine(), K.highlightSelectionMatches(), K.indentUnit.of("    "), K.EditorState.tabSize.of(4),
      K.search({ top: true }),
      K.keymap.of([
        { key: "Mod-s", run: () => { saveAll(true); return true; } },
        { key: "Mod-l", run: K.gotoLine },
        { key: "Mod-/", run: K.toggleComment },
        ...K.closeBracketsKeymap, ...K.defaultKeymap, ...K.searchKeymap, ...K.historyKeymap,
        ...K.foldKeymap, ...K.completionKeymap, ...K.lintKeymap, K.indentWithTab,
      ]),
      lang, addsField, edTheme,
      K.EditorView.updateListener.of((u) => onEditorUpdate(relOfView(u.view), u)),
    ];
  }

  /* ---- the project being worked on ------------------------------------- */
  let ide = null; // { path, name, tree, docs: Map, open: [], cur, busy, built }
  const view = $("viewIde");

  function setStatus(kind, text) {
    const st = $("ideSt");
    st.dataset.kind = kind; // ok | busy | bad | idle
    $("ideStT").textContent = text;
    $("ideStop").classList.toggle("hidden", kind !== "busy");
  }
  function term(line, cls) {
    const pre = $("ideTerm");
    const span = document.createElement("span");
    span.className = cls || (/^==>/.test(line) ? "b" : /error|failed|refused/i.test(line) ? "r" : /\bok\b|passed|built/i.test(line) ? "g" : "");
    span.textContent = line + "\n";
    pre.appendChild(span);
    // Bounded: a long build must not grow the page without end.
    while (pre.childNodes.length > 2000) pre.removeChild(pre.firstChild);
    pre.scrollTop = pre.scrollHeight;
  }
  /* The compiler's own errors, read from a failed build: rustc prints
   *   error[E0425]: cannot find value `x` in this scope
   *     --> src/lib.rs:88:52
   * and each becomes a problem with a file and a line (K-973: the whole
   * output used to sit in one block nobody could click). */
  const FIX = '<svg width="12" height="12" viewBox="0 0 16 16" fill="none"><path d="M8 1.6l1.4 3.5 3.5 1.4-3.5 1.4L8 11.4 6.6 7.9 3.1 6.5l3.5-1.4z" fill="currentColor"/></svg>';
  function parseDiags(text) {
    const out = [];
    const rx = /^(error|warning)(\[[A-Z]\d+\])?: (.+)\n\s*--> ([^\s:][^:\n]*):(\d+):(\d+)/gm;
    let m;
    while ((m = rx.exec(String(text || "")))) {
      // The snippet rustc drew under it, up to the next blank line.
      const rest = String(text).slice(m.index);
      const end = rest.search(/\n\s*\n/);
      out.push({ sev: m[1], code: (m[2] || "").replace(/[[\]]/g, ""), msg: m[3], file: m[4].replace(/^\.\//, ""), line: +m[5], col: +m[6], text: end > 0 ? rest.slice(0, end) : rest });
    }
    return out;
  }
  function problems(list, notes) {
    const box = $("ideProbs");
    notes = notes || [];
    const diags = [];
    for (const p of list) for (const d of parseDiags(p.message)) diags.push(d);
    ide.diags = diags;
    const errs = diags.filter((d) => d.sev === "error").length;
    $("ideProbN").textContent = String(diags.length ? errs || diags.length : list.length);
    const row = (d, i) => `<div class="ip-diag ${d.sev}"><div class="ip-row"><button type="button" class="ip-go" data-d="${i}"><i></i><b>${esc(d.file)}:${d.line}</b><span>${esc(d.msg)}</span>${d.code ? `<em>${esc(d.code)}</em>` : ""}</button>${d.sev === "error" ? `<button type="button" class="ip-fix" data-fix="${i}">${FIX}Fix with AI</button>` : ""}</div><details><summary>What the compiler said</summary><pre>${esc(d.text)}</pre></details></div>`;
    ide.lastFail = list.map((p) => p.message).join("\n");
    box.innerHTML = (diags.length ? diags.map(row).join("")
      : list.length ? list.map((p) => `<div class="ip-prob"><b>${esc(p.stage)}</b>${p.stage !== "ask" ? `<button type="button" class="ip-fix" data-fix="-1">${FIX}Fix with AI</button>` : ""}<pre>${esc(p.message)}</pre></div>`).join("")
      : '<p class="ip-ok">No problems. It builds, imports only Krate, and runs.</p>') +
      notes.map((n) => `<div class="ip-prob ip-note"><b>note</b><pre>${esc(n)}</pre></div>`).join("");
    markDiags();
    if (list.length) panelTab("prob");
  }
  $("ideProbs").addEventListener("click", (e) => {
    const b = e.target.closest(".ip-go"); if (!b || !ide || !ide.diags) return;
    const d = ide.diags[+b.dataset.d]; if (!d) return;
    // A file the compiler named that is not in the project: said right on
    // the row, where the person is looking.
    if (!ide.tree.some((t) => t.rel === d.file)) {
      const row = b.closest(".ip-diag");
      if (row && !row.querySelector(".ip-miss")) row.insertAdjacentHTML("beforeend", `<p class="ip-miss">${esc(d.file)} is not in this project, so it cannot be opened here.</p>`);
    }
    goTo(d.file, d.line, d.col);
  });
  async function goTo(rel, line, col) {
    if (!ide.tree.some((t) => t.rel === rel)) { term(`${rel} is not in this project`, "r"); toast(`${rel} is not in this project`); return; }
    if (ide.cur !== rel || !edView) await openFile(rel);
    if (!edView) return;
    const doc = edView.state.doc;
    const ln = doc.line(Math.max(1, Math.min(line || 1, doc.lines)));
    const at = Math.min(ln.from + Math.max(0, (col || 1) - 1), ln.to);
    edView.dispatch({ selection: { anchor: at }, scrollIntoView: true });
    edView.focus();
  }
  // The errors drawn in the editor too (a mark in the gutter, a line under
  // the code): the linter reads ide.diags, so it only needs to run again.
  // forceLinting only hurries a run that is already due, so the marks are
  // set directly: a failed build shows its errors before anyone types.
  function markDiags() {
    if (!CM || !ide) return;
    for (const d of ide.docs.values()) if (d.view) { try { d.view.dispatch(CM.setDiagnostics(d.view.state, krateLint(d.view))); } catch (e) {} }
  }
  function panelTab(which) {
    view.querySelectorAll(".ip-tabs button[data-ip]").forEach((b) => b.classList.toggle("on", b.dataset.ip === which));
    $("ideTerm").classList.toggle("hidden", which !== "term");
    $("ideProbs").classList.toggle("hidden", which !== "prob");
    $("ideFind").classList.toggle("hidden", which !== "find");
    $("ideApi").classList.toggle("hidden", which !== "api");
    $("ideAns").classList.toggle("hidden", which !== "ans");
    if (which === "api") { paintApi(); setTimeout(() => $("ideApiIn").focus(), 30); }
    $("ideMain").classList.remove("pmin");
    if (which === "find") setTimeout(() => $("ideFindIn").focus(), 30);
  }

  /* The Krate API, every call a guest can make, grouped by module; a click
   * puts the call at the cursor. The same list the AI is handed. */
  function paintApi() {
    const out = $("ideApiOut"); if (!out) return;
    const q = ($("ideApiIn").value || "").trim().toLowerCase();
    const fns = sdk().functions.filter((f) => !q || f.signature.toLowerCase().includes(q));
    if (!sdk().functions.length) { out.innerHTML = '<p class="ip-ok">This engine does not describe its API yet.</p>'; return; }
    // Grouped by module, each heading once, in the order modules first appear.
    const groups = new Map();
    for (const f of fns.slice(0, 400)) {
      const group = f.method ? `Methods on ${f.receiver}` : f.module;
      if (!groups.has(group)) groups.set(group, []);
      groups.get(group).push(f);
    }
    let html = "";
    for (const [group, list] of groups) {
      html += `<div class="api-mod">${esc(group)}</div>`;
      for (const f of list) html += `<button type="button" class="ip-hit api-fn" data-i="${sdk().functions.indexOf(f)}" title="Put it at the cursor"><span>${esc(f.signature)}</span></button>`;
    }
    out.innerHTML = html || '<p class="ip-ok">No call matches.</p>';
  }
  $("ideApiIn").addEventListener("input", paintApi);
  $("ideApiOut").addEventListener("click", (e) => {
    const b = e.target.closest(".api-fn"); if (!b || !edView) return;
    const f = sdk().functions[+b.dataset.i]; if (!f) return;
    const text = f.method ? `.${f.name}(` : `${f.module}::${f.name}(`;
    const sel = edView.state.selection.main;
    edView.dispatch({ changes: { from: sel.from, to: sel.to, insert: text }, selection: { anchor: sel.from + text.length } });
    edView.focus();
  });

  /* Search every file in the project (Cmd-Shift-F). Text files are read
   * once and kept; an open file is searched as it is now, unsaved edits
   * and all. */
  let findT = 0;
  async function findAll(q) {
    const out = $("ideFindOut");
    if (!ide || !q) { out.innerHTML = ""; return; }
    const isOpen = (rel) => !!(ide.docs.get(rel) && ide.docs.get(rel).state);
    const text = ide.tree.filter((e) => !e.dir && /\.(rs|toml|md|txt|json|wit)$/i.test(e.rel));
    const files = text.filter((e) => isOpen(e.rel) || (e.size || 0) < 400000);
    const skipped = text.length - files.length;
    const needle = q.toLowerCase(); const hits = [];
    for (const f of files) {
      // Closed files are read as they are now: a cached copy went stale when
      // the AI or another editor changed one.
      let text = ide.docs.get(f.rel) && ide.docs.get(f.rel).state ? textOf(ide.docs.get(f.rel)) : null;
      if (text == null) { try { text = await call("ide_read", { path: ide.path, rel: f.rel }); } catch (e) { continue; } }
      text.split("\n").forEach((l, i) => { const at = l.toLowerCase().indexOf(needle); if (at >= 0 && hits.length < 300) hits.push({ rel: f.rel, line: i + 1, col: at + 1, l }); });
    }
    if ($("ideFindIn").value.trim() !== q) return;
    out.innerHTML = hits.length ? hits.map((h, i) => {
      const a = Math.max(0, h.col - 41), s = h.l.slice(a, h.col - 1), m = h.l.slice(h.col - 1, h.col - 1 + q.length), r = h.l.slice(h.col - 1 + q.length, h.col + 80);
      return `<button type="button" class="ip-hit" data-h="${i}"><b>${esc(h.rel)}:${h.line}</b><span>${a ? "…" : ""}${esc(s.trimStart())}<mark>${esc(m)}</mark>${esc(r)}</span></button>`;
    }).join("") + (hits.length >= 300 ? '<p class="ip-ok">The first 300 matches.</p>' : "") : '<p class="ip-ok">Nothing matches.</p>';
    if (skipped) out.insertAdjacentHTML("beforeend", `<p class="ip-ok">${skipped} file${skipped === 1 ? " is" : "s are"} over 400 KB and ${skipped === 1 ? "was" : "were"} not searched; open ${skipped === 1 ? "it" : "one"} to search it.</p>`);
    out._hits = hits;
  }
  $("ideFindIn").addEventListener("input", () => { clearTimeout(findT); findT = setTimeout(() => findAll($("ideFindIn").value.trim()), 180); });
  $("ideFindOut").addEventListener("click", (e) => { const b = e.target.closest(".ip-hit"); if (!b) return; const h = $("ideFindOut")._hits[+b.dataset.h]; if (h) goTo(h.rel, h.line, h.col); });

  try {
    tauri.event.listen("ide-line", (e) => {
      const p = e.payload || {};
      if (!ide || !p.line) return;
      // One project is open at a time. While it is working, its lines are
      // its own whatever spelling of the path they carry (a project opened
      // from an app's Code pane can arrive under another one, K-973).
      if (!ide.busy && p.path && p.path !== ide.path && !p.path.endsWith("/" + ide.name)) return;
      term(String(p.line));
    });
  } catch (e) { /* no events: the result still arrives */ }

  /* ---- the tree --------------------------------------------------------- */
  const ICON = {
    dir: '<svg width="14" height="14" viewBox="0 0 16 16" fill="none"><path d="M2.5 4.6c0-.6.5-1.1 1.1-1.1h2.8l1.4 1.5h4.6c.6 0 1.1.5 1.1 1.1v5.8c0 .6-.5 1.1-1.1 1.1H3.6c-.6 0-1.1-.5-1.1-1.1z" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round"/></svg>',
    rs: '<svg width="14" height="14" viewBox="0 0 16 16" fill="none"><path d="M5.6 4.6L2.4 8l3.2 3.4M10.4 4.6L13.6 8l-3.2 3.4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>',
    img: '<svg width="14" height="14" viewBox="0 0 16 16" fill="none"><circle cx="8" cy="8" r="5.8" stroke="currentColor" stroke-width="1.4"/><path d="M8 2.2a5.8 5.8 0 0 1 0 11.6z" fill="currentColor"/></svg>',
    file: '<svg width="14" height="14" viewBox="0 0 16 16" fill="none"><path d="M4 2.5h5l3 3v8H4z M9 2.5v3h3" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round"/></svg>',
  };
  const iconFor = (e) => e.dir ? ICON.dir : /\.rs$/.test(e.rel) ? ICON.rs : /\.(png|jpe?g|gif|webp|svg|ico)$/i.test(e.rel) ? ICON.img : ICON.file;
  async function loadTree() {
    const empty = $("ideEmpty");
    try { ide.tree = (await call("ide_tree", { path: ide.path })) || []; empty.textContent = "Choose a file on the left."; }
    catch (e) { ide.tree = []; term(String(e), "r"); empty.textContent = String(e); setStatus("bad", "Project not found"); }
    paintTree();
  }
  function paintTree() {
    const box = $("ideTree");
    const hiddenUnder = [...ide.collapsed];
    box.innerHTML = `<div class="th2"><span>${esc(ide.name.toUpperCase())}</span><button type="button" class="th-new" data-tnew="" title="New file">${PLUS}</button></div>` + ide.tree.map((e) => {
      const depth = e.rel.split("/").length - 1;
      const gone = hiddenUnder.some((d) => e.rel.startsWith(d + "/"));
      if (gone) return "";
      const name = e.rel.split("/").pop();
      const doc = ide.docs.get(e.rel);
      const on = !e.dir && e.rel === ide.cur;
      return `<button type="button" class="tn${on ? " on" : ""}${e.dir ? " dir" : ""}${e.dir && ide.collapsed.has(e.rel) ? " shut" : ""}" style="--lv:${depth}" data-rel="${esc(e.rel)}" data-dir="${e.dir ? 1 : ""}" title="${esc(e.rel)}"${e.dir ? ` aria-expanded="${!ide.collapsed.has(e.rel)}"` : ""}>${iconFor(e)}<span>${esc(name)}</span>${doc && doc.dirty ? '<i class="md" title="Not saved"></i>' : ""}</button>`;
    }).join("");
  }
  $("ideTree").addEventListener("click", (e) => {
    const nb = e.target.closest("[data-tnew]"); if (nb && ide) { newFileIn(nb.dataset.tnew); return; }
    const b = e.target.closest(".tn"); if (!b || !ide) return;
    const rel = b.dataset.rel;
    if (b.dataset.dir) { ide.collapsed.has(rel) ? ide.collapsed.delete(rel) : ide.collapsed.add(rel); paintTree(); return; }
    openFile(rel);
  });

  /* ---- files: new, rename, delete ---------------------------------------- */
  const PLUS = '<svg width="13" height="13" viewBox="0 0 16 16" fill="none"><path d="M8 3.4v9.2M3.4 8h9.2" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/></svg>';
  // A small menu at the pointer, for a file or folder in the tree.
  let menuEl = null;
  function closeMenu() { if (menuEl) { menuEl.remove(); menuEl = null; } }
  document.addEventListener("pointerdown", (e) => { if (menuEl && !menuEl.contains(e.target)) closeMenu(); }, true);
  document.addEventListener("keydown", (e) => { if (menuEl && e.key === "Escape") { e.preventDefault(); e.stopPropagation(); closeMenu(); } }, true);
  function menu(x, y, items) {
    closeMenu();
    menuEl = document.createElement("div"); menuEl.className = "ide-menu"; menuEl.setAttribute("role", "menu");
    for (const it of items) {
      if (it === "sep") { menuEl.appendChild(Object.assign(document.createElement("div"), { className: "sep" })); continue; }
      const b = document.createElement("button"); b.type = "button"; b.textContent = it.label; b.setAttribute("role", "menuitem");
      if (it.danger) b.className = "danger";
      b.addEventListener("click", () => { closeMenu(); it.run(); });
      menuEl.appendChild(b);
    }
    document.body.appendChild(menuEl);
    const w = menuEl.offsetWidth, h = menuEl.offsetHeight;
    menuEl.style.left = Math.min(x, innerWidth - w - 8) + "px"; menuEl.style.top = Math.min(y, innerHeight - h - 8) + "px";
    const first = menuEl.querySelector("button"); if (first) first.focus();
    menuEl.addEventListener("keydown", (e) => {
      const items = [...menuEl.querySelectorAll("button")], at = items.indexOf(document.activeElement);
      if (e.key === "ArrowDown" || e.key === "ArrowUp") { e.preventDefault(); items[(at + (e.key === "ArrowDown" ? 1 : items.length - 1)) % items.length].focus(); }
    });
  }
  $("ideTree").addEventListener("contextmenu", (e) => {
    const b = e.target.closest(".tn"); if (!b || !ide) return;
    e.preventDefault();
    const rel = b.dataset.rel, dir = !!b.dataset.dir;
    const folder = dir ? rel : rel.includes("/") ? rel.slice(0, rel.lastIndexOf("/")) : "";
    treeMenu(rel, dir, e.clientX, e.clientY);
  });
  function treeMenu(rel, dir, x, y) {
    const folder = dir ? rel : rel.includes("/") ? rel.slice(0, rel.lastIndexOf("/")) : "";
    menu(x, y, [
      { label: "New file here", run: () => newFileIn(folder) },
      { label: "New folder here", run: () => newFolderIn(folder) },
      { label: "Rename", run: () => renameIn(rel) },
      "sep",
      { label: "Delete", danger: true, run: () => deleteIn(rel, dir) },
    ]);
  }
  // The tree by keyboard: arrows move, left and right fold, Enter opens,
  // F2 renames, Delete deletes, the menu key (or Shift+F10) opens the menu.
  $("ideTree").addEventListener("keydown", (e) => {
    const b = e.target.closest(".tn"); if (!b || !ide) return;
    const rows = [...$("ideTree").querySelectorAll(".tn")], at = rows.indexOf(b);
    const rel = b.dataset.rel, dir = !!b.dataset.dir;
    if (e.key === "ArrowDown" || e.key === "ArrowUp") { e.preventDefault(); const n = rows[at + (e.key === "ArrowDown" ? 1 : -1)]; if (n) n.focus(); }
    else if (e.key === "ArrowRight" && dir && ide.collapsed.has(rel)) { e.preventDefault(); ide.collapsed.delete(rel); paintTree(); focusRow(rel); }
    else if (e.key === "ArrowLeft" && dir && !ide.collapsed.has(rel)) { e.preventDefault(); ide.collapsed.add(rel); paintTree(); focusRow(rel); }
    else if (e.key === "ArrowLeft" && rel.includes("/")) { e.preventDefault(); focusRow(rel.slice(0, rel.lastIndexOf("/"))); }
    else if (e.key === "F2") { e.preventDefault(); renameIn(rel); }
    else if (e.key === "Delete" || (e.key === "Backspace" && (e.metaKey || e.ctrlKey))) { e.preventDefault(); deleteIn(rel, dir); }
    else if (e.key === "ContextMenu" || (e.key === "F10" && e.shiftKey)) { e.preventDefault(); const r = b.getBoundingClientRect(); treeMenu(rel, dir, r.left + 24, r.bottom); }
  });
  function focusRow(rel) { const r = $("ideTree").querySelector(`.tn[data-rel="${CSS.escape(rel)}"]`); if (r) r.focus(); }
  // A name typed right in the tree, where the file will be.
  function inlineName(placeholder, value, after) {
    return new Promise((resolve) => {
      const box = $("ideTree");
      const f = document.createElement("form"); f.className = "tn-in"; f.autocomplete = "off";
      f.innerHTML = `<input type="text" spellcheck="false" placeholder="${esc(placeholder)}" aria-label="${esc(placeholder)}">`;
      const input = f.querySelector("input"); input.value = value || "";
      if (after) after.after(f); else box.appendChild(f);
      input.focus(); if (value) input.setSelectionRange(0, value.lastIndexOf(".") > 0 ? value.lastIndexOf(".") : value.length);
      let done = false;
      const finish = (v) => { if (done) return; done = true; f.remove(); resolve(v); };
      f.addEventListener("submit", (e) => { e.preventDefault(); finish(input.value.trim()); });
      input.addEventListener("keydown", (e) => { if (e.key === "Escape") { e.preventDefault(); finish(""); } });
      // Clicking away keeps what was typed, like pressing Enter.
      input.addEventListener("blur", () => setTimeout(() => finish(input.value.trim()), 120));
    });
  }
  async function newFileIn(folder) {
    if (!ide) return;
    const anchor = folder ? $("ideTree").querySelector(`.tn[data-rel="${CSS.escape(folder)}"]`) : null;
    const name = await inlineName(folder ? `New file in ${folder}/` : "New file, like src/helpers.rs", "", anchor);
    if (!name) return;
    const rel = (folder && !name.includes("/") ? folder + "/" : "") + name.replace(/^\/+/, "");
    if (ide.tree.some((t) => t.rel === rel)) { term(`${rel} already exists`, "r"); openFile(rel); return; }
    try { await call("ide_write", { path: ide.path, rel, text: "" }); }
    catch (err) { term(`could not make ${rel}: ${err}`, "r"); return; }
    term(`==> made ${rel}`, "m");
    await loadTree(); await openFile(rel);
    // A new Rust file is only built once lib.rs names it.
    const m = rel.match(/^src\/(?:.*\/)?([a-z_][a-z0-9_]*)\.rs$/);
    if (m && !/^src\/(lib|main|bindings)\.rs$/.test(rel) && rel.split("/").length === 2 && ide.tree.some((t) => t.rel === "src/lib.rs")) {
      const k = await ask(`Add mod ${m[1]}; to src/lib.rs, so this file is part of the build?`, [["add", "Add it", true], ["no", "Not now"]]);
      if (k === "add") await addMod(m[1]);
    }
  }
  // A folder holds files, so a new folder starts with its first file.
  async function newFolderIn(parent) {
    if (!ide) return;
    const anchor = parent ? $("ideTree").querySelector(`.tn[data-rel="${CSS.escape(parent)}"]`) : null;
    const name = await inlineName(parent ? `New folder in ${parent}/` : "New folder, like src/ui", "", anchor);
    if (!name) return;
    const folder = ((parent && !name.includes("/") ? parent + "/" : "") + name).replace(/^\/+|\/+$/g, "");
    if (ide.tree.some((t) => t.rel === folder)) { term(`${folder} already exists`, "r"); toast(`${folder} already exists`); return; }
    await newFileIn(folder);
  }
  async function addMod(name) {
    let doc = ide.docs.get("src/lib.rs");
    if (!doc || !doc.state) { await openFile("src/lib.rs"); doc = ide.docs.get("src/lib.rs"); }
    if (!doc || !doc.state) return;
    const text = textOf(doc);
    if (new RegExp(`^\\s*(pub\\s+)?mod\\s+${name}\\s*;`, "m").test(text)) return;
    // After the last mod line; else after the attributes and uses at the top.
    const lines = text.split("\n"); let at = -1;
    lines.forEach((l, i) => { if (/^\s*(pub\s+)?mod\s+\w+\s*;/.test(l)) at = i; });
    if (at < 0) lines.forEach((l, i) => { if (i < 60 && /^\s*(#!\[|extern crate|use )/.test(l)) at = i; });
    const pos = at < 0 ? 0 : doc.state.doc.line(at + 1).to;
    applyTo(doc, { changes: { from: pos, insert: (at < 0 ? "" : "\n") + `mod ${name};` + (at < 0 ? "\n" : "") } });
    await saveOne("src/lib.rs"); paintTabs(); paintTree();
    build(`mod ${name} added`);
  }
  async function renameIn(rel) {
    const b = $("ideTree").querySelector(`.tn[data-rel="${CSS.escape(rel)}"]`);
    const base = rel.split("/").pop();
    const name = await inlineName("New name", base, b);
    if (!name || name === base) return;
    const to = name.includes("/") ? name : (rel.includes("/") ? rel.slice(0, rel.lastIndexOf("/") + 1) : "") + name;
    await saveAll(false);
    try { await call("ide_rename", { path: ide.path, from: rel, to }); }
    catch (err) { term(String(err), "r"); return; }
    term(`==> renamed ${rel} to ${to}`, "m");
    // Open files follow the rename (a folder takes its files with it).
    for (const [r, d] of [...ide.docs]) {
      const moved = r === rel ? to : r.startsWith(rel + "/") ? to + r.slice(rel.length) : null;
      if (!moved) continue;
      ide.docs.delete(r); ide.docs.set(moved, d); dropDraft(r);
      ide.open = ide.open.map((o) => (o === r ? moved : o));
      if (ide.cur === r) ide.cur = moved;
    }
    await loadTree(); paintTabs(); showCur();

    // src/a.rs named in lib.rs as `mod a;`: offer to follow the rename, or
    // the next build fails on a module that is not there.
    const was = rel.match(/^src\/([a-z_][a-z0-9_]*)\.rs$/), now = to.match(/^src\/([a-z_][a-z0-9_]*)\.rs$/);
    const lib = ide.docs.get("src/lib.rs");
    let libText = lib && lib.state ? textOf(lib) : null;
    if (was && libText == null) { try { libText = await call("ide_read", { path: ide.path, rel: "src/lib.rs" }); } catch (e) { libText = null; } }
    const modRx = was ? new RegExp(`^(\\s*(?:pub\\s+)?mod\\s+)${was[1]}(\\s*;)`, "m") : null;
    if (was && libText != null && modRx.test(libText)) {
      const k = await ask(now ? `Change mod ${was[1]}; to mod ${now[1]}; in src/lib.rs, so it still builds?` : `src/lib.rs still says mod ${was[1]};. Take that line out, so it still builds?`, [["fix", now ? "Change it" : "Take it out", true], ["no", "Not now"]]);
      if (k === "fix") {
        // In the editor when lib.rs is open, on disk when it is not: the
        // renamed file stays the one in front.
        const d = ide.docs.get("src/lib.rs");
        const t = d && d.state ? textOf(d) : libText, m = t.match(modRx);
        if (m) {
          const from = m.index, end = from + m[0].length, cut = now ? end : Math.min(t.length, end + (t[end] === "\n" ? 1 : 0));
          const insert = now ? `${m[1]}${now[1]}${m[2]}` : "";
          if (d && d.state) { applyTo(d, { changes: { from, to: cut, insert } }); await saveOne("src/lib.rs"); }
          else { try { await call("ide_write", { path: ide.path, rel: "src/lib.rs", text: t.slice(0, from) + insert + t.slice(cut) }); } catch (err) { term(`could not change src/lib.rs: ${err}`, "r"); return; } }
          paintTabs(); paintTree();
          build("mod line changed");
        }
      }
    }
  }
  async function deleteIn(rel, dir) {
    const k = await ask(`Delete ${rel}${dir ? " and everything in it" : ""}? It is moved to Studio's backups, so it can be got back.`, [["del", "Delete", true], ["no", "Cancel"]]);
    if (k !== "del") return;
    try { await call("ide_delete", { path: ide.path, rel }); }
    catch (err) { term(String(err), "r"); return; }
    term(`==> deleted ${rel} (kept in Studio's ide-backups)`, "m");
    for (const [r, d] of [...ide.docs]) {
      if (r !== rel && !r.startsWith(rel + "/")) continue;
      if (d.view) { d.view.destroy(); d.host.remove(); }
      ide.docs.delete(r); dropDraft(r);
      ide.open = ide.open.filter((o) => o !== r);
      if (ide.cur === r) ide.cur = ide.open[ide.open.length - 1] || null;
    }
    await loadTree(); paintTabs(); showCur();
  }

  /* Quick open (Cmd-P): any file in the project by a few letters of its name. */
  function quickOpen() {
    if (!ide) return;
    const files = ide.tree.filter((t) => !t.dir).map((t) => t.rel);
    const box = document.createElement("div"); box.className = "ide-qo";
    box.innerHTML = '<input type="text" placeholder="Go to a file" spellcheck="false" aria-label="Go to a file"><div class="qo-list" role="listbox"></div>';
    $("ideEd").appendChild(box);
    const input = box.querySelector("input"), list = box.querySelector(".qo-list");
    let hits = [], sel = 0;
    const score = (rel, q) => { let i = 0, s = 0; const r = rel.toLowerCase(); for (const ch of q) { const j = r.indexOf(ch, i); if (j < 0) return -1; s += j - i; i = j + 1; } return s + rel.length / 100; };
    const paint = () => {
      const q = input.value.trim().toLowerCase();
      hits = files.map((f) => [f, q ? score(f, q) : 0]).filter((x) => x[1] >= 0).sort((a, b) => a[1] - b[1]).slice(0, 12).map((x) => x[0]);
      sel = Math.min(sel, Math.max(0, hits.length - 1));
      list.innerHTML = hits.map((f, i) => `<button type="button" class="${i === sel ? "on" : ""}" data-i="${i}"><b>${esc(f.split("/").pop())}</b><small>${esc(f)}</small></button>`).join("") || '<p>No file matches.</p>';
    };
    const close = () => box.remove();
    const go = (i) => { const f = hits[i]; close(); if (f) openFile(f); };
    input.addEventListener("input", () => { sel = 0; paint(); });
    input.addEventListener("keydown", (e) => {
      if (e.key === "Escape") { e.preventDefault(); close(); if (edView) edView.focus(); }
      else if (e.key === "ArrowDown") { e.preventDefault(); sel = Math.min(hits.length - 1, sel + 1); paint(); }
      else if (e.key === "ArrowUp") { e.preventDefault(); sel = Math.max(0, sel - 1); paint(); }
      else if (e.key === "Enter") { e.preventDefault(); go(sel); }
    });
    input.addEventListener("blur", () => setTimeout(close, 150));
    list.addEventListener("mousedown", (e) => { const b = e.target.closest("[data-i]"); if (b) { e.preventDefault(); go(+b.dataset.i); } });
    paint(); input.focus();
  }

  /* ---- the editor ----------------------------------------------------- */
  // One CodeMirror view; each open file keeps its own state, so its undo
  // history, cursor and scroll survive switching tabs (K-973).
  const ed = $("ideEd");
  ed.insertAdjacentHTML("beforeend", '<div class="ide-bar hidden" id="ideBar" role="status"><span class="t"></span><span class="grow"></span></div><div class="ide-code hidden" id="ideCode"></div><div class="ide-msg hidden" id="ideMsg"></div>');
  let edView = null;
  const textOf = (doc) => (doc && doc.state ? doc.state.doc.toString() : (doc && doc.text) || "");
  function onEditorUpdate(rel, u) {
    const doc = ide && ide.docs.get(rel); if (!doc) return;
    doc.state = u.state;
    if (!u.docChanged) return;
    const was = doc.dirty;
    doc.dirty = textOf(doc) !== doc.saved;
    keepDraft(rel, doc);
    if (was !== doc.dirty) { paintTabs(); paintTree(); }
  }
  // A change to a file: through its view when it has one (the view's
  // listener keeps doc.state), to its state when it has not been shown.
  function applyTo(doc, spec) {
    if (doc.view) doc.view.dispatch(spec);
    else doc.state = doc.state.update(spec).state;
  }
  function newState(rel, text) { return CM.EditorState.create({ doc: text, extensions: extensionsFor(rel) }); }
  // Replace a file's text without losing its undo history.
  function replaceText(rel, doc, text) {
    const len = doc.state.doc.length;
    const tr = { changes: { from: 0, to: len, insert: text } };
    applyTo(doc, tr);
  }

  /* Unsaved work is kept as a draft as you type, so closing the window, a
   * crash or a reload never loses it; it comes back the next time the file
   * opens, marked as not saved (K-973). */
  const DRAFTS = "krate-ide-drafts";
  const drafts = () => { try { return JSON.parse(localStorage.getItem(DRAFTS) || "{}"); } catch (e) { return {}; } };
  const draftT = new Map();
  function keepDraft(rel, doc) {
    clearTimeout(draftT.get(rel));
    const path = ide.path;
    draftT.set(rel, setTimeout(() => {
      draftT.delete(rel);
      if (!ide || ide.path !== path) return;
      try {
        const all = drafts(); const mine = all[ide.path] || {};
        if (doc.dirty) mine[rel] = textOf(doc); else delete mine[rel];
        if (Object.keys(mine).length) all[ide.path] = mine; else delete all[ide.path];
        localStorage.setItem(DRAFTS, JSON.stringify(all));
      } catch (e) { /* a full or blocked store: the save is still the save */ }
    }, 300));
  }
  function dropDraft(rel) {
    try { const all = drafts(); if (all[ide.path]) { delete all[ide.path][rel]; if (!Object.keys(all[ide.path]).length) delete all[ide.path]; localStorage.setItem(DRAFTS, JSON.stringify(all)); } } catch (e) {}
  }

  /* A question above the editor, answered with one of its buttons. */
  let askOpen = null;
  function ask(text, choices) {
    const bar = $("ideBar");
    // A question replaced by another is answered "nothing chosen", so
    // whoever waits on it moves on (the disk check stopped for good, K-985).
    if (askOpen) { const r = askOpen; askOpen = null; r(null); }
    return new Promise((res) => {
      askOpen = res;
      bar.querySelector(".t").textContent = text;
      bar.querySelectorAll("button").forEach((b) => b.remove());
      for (const [key, label, primary] of choices) {
        const b = document.createElement("button"); b.type = "button"; b.textContent = label;
        if (primary) b.className = "pri";
        b.addEventListener("click", () => { if (askOpen === res) askOpen = null; bar.classList.add("hidden"); res(key); });
        bar.appendChild(b);
      }
      bar.classList.remove("hidden");
    });
  }

  async function openFile(rel) {
    if (!ide) return;
    try { await loadCM(); } catch (err) { $("ideMsg").textContent = `${String(err.message || err)}. Open the file again to retry.`; $("ideMsg").classList.remove("hidden"); $("ideEmpty").classList.add("hidden"); return; }
    let doc = ide.docs.get(rel);
    let recovered = false;
    if (!doc) {
      try {
        const text = await call("ide_read", { path: ide.path, rel });
        const draft = (drafts()[ide.path] || {})[rel];
        recovered = typeof draft === "string" && draft !== text;
        doc = { saved: text, dirty: recovered, error: null, state: newState(rel, recovered ? draft : text) };
      } catch (err) {
        doc = { saved: "", dirty: false, error: String(err), state: null };
      }
      ide.docs.set(rel, doc);
    }
    if (!ide.open.includes(rel)) ide.open.push(rel);
    ide.cur = rel;
    showDoc();
    if (ide.diags && ide.diags.some((d) => d.file === rel)) markDiags();
    paintTabs(); paintTree();
    // Asked without holding the project up: it still opens and builds.
    if (recovered) {
      ask(`Your unsaved changes to ${rel.split("/").pop()} were kept from last time.`, [["keep", "Keep them", true], ["drop", "Discard"]]).then((k) => {
        if (k === "drop") { replaceText(rel, doc, doc.saved); doc.dirty = false; dropDraft(rel); paintTabs(); paintTree(); }
      });
    }
  }
  // The current tab, loaded if it was listed but never opened (manifest.toml
  // joins the tabs when a project opens, before anyone looks at it).
  function showCur() {
    if (ide && ide.cur && !ide.docs.get(ide.cur)) { openFile(ide.cur); return; }
    showDoc();
  }
  function showDoc() {
    const doc = ide && ide.cur ? ide.docs.get(ide.cur) : null;
    const code = $("ideCode"), msg = $("ideMsg"), empty = $("ideEmpty");
    empty.classList.toggle("hidden", !!doc);
    code.classList.toggle("hidden", !doc || !!doc.error);
    msg.classList.toggle("hidden", !doc || !doc.error);
    if (!doc) return;
    if (doc.error) { msg.textContent = `${ide.cur}: ${doc.error}`; return; }
    // One view per open file, shown and hidden: each keeps its own scroll,
    // and nothing from one file (a hover card, a completion) is ever laid
    // out against another's text.
    // Each in a box of its own: CodeMirror rewrites its editor's classes on
    // every update and forces it to display:flex, so the box is what hides.
    for (const d of ide.docs.values()) if (d.host && d !== doc) d.host.hidden = true;
    if (!doc.view) {
      doc.host = document.createElement("div"); doc.host.className = "kr-edhost";
      code.appendChild(doc.host);
      doc.view = new CM.EditorView({ state: doc.state, parent: doc.host });
    }
    doc.host.hidden = false;
    edView = doc.view;
    requestAnimationFrame(() => { try { edView.focus(); } catch (e) {} });
  }
  function paintTabs() {
    const box = $("ideTabs");
    box.innerHTML = (ide ? ide.open : []).map((rel) => {
      const doc = ide.docs.get(rel);
      return `<span class="ide-tab${rel === ide.cur ? " on" : ""}" role="tab" aria-selected="${rel === ide.cur}" data-rel="${esc(rel)}" title="${esc(rel)}">${/\.rs$/.test(rel) ? ICON.rs : ICON.file}${esc(rel.split("/").pop())}${doc && doc.dirty ? '<i class="md"></i>' : ""}<button type="button" class="x" data-x="${esc(rel)}" title="Close" aria-label="Close ${esc(rel)}"><svg width="10" height="10" viewBox="0 0 16 16" fill="none"><path d="M4.2 4.2l7.6 7.6M11.8 4.2l-7.6 7.6" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/></svg></button></span>`;
    }).join("");
  }
  $("ideTabs").addEventListener("click", async (e) => {
    if (!ide) return;
    const x = e.target.closest("[data-x]");
    if (x) {
      const rel = x.dataset.x; const doc = ide.docs.get(rel);
      // A tab with changes asks; it used to save them without a word.
      if (doc && doc.dirty) {
        const k = await ask(`Save your changes to ${rel.split("/").pop()} before closing it?`, [["save", "Save", true], ["drop", "Don't save"], ["cancel", "Cancel"]]);
        if (k === "cancel" || !k) return;
        if (k === "save" && !(await saveOne(rel))) return;
        if (k === "drop") { dropDraft(rel); if (doc.view) { doc.view.destroy(); doc.host.remove(); } ide.docs.delete(rel); }
      }
      ide.open = ide.open.filter((r) => r !== rel);
      if (ide.cur === rel) ide.cur = ide.open[ide.open.length - 1] || null;
      showCur(); paintTabs(); paintTree(); return;
    }
    const t = e.target.closest("[data-rel]"); if (t && t.dataset.rel !== ide.cur) openFile(t.dataset.rel);
  });

  async function saveOne(rel) {
    const doc = ide.docs.get(rel); if (!doc || !doc.dirty || doc.error) return true;
    const text = textOf(doc);
    try {
      await call("ide_write", { path: ide.path, rel, text });
      doc.saved = text; doc.dirty = textOf(doc) !== doc.saved; dropDraft(rel); return true;
    } catch (err) { term(`could not save ${rel}: ${err}`, "r"); setStatus("bad", `Could not save ${rel.split("/").pop()}`); toast(`Could not save ${rel.split("/").pop()}`); return false; }
  }
  async function saveAll(thenBuild) {
    if (!ide) return;
    let any = false, failed = false;
    for (const [rel, doc] of ide.docs) if (doc.dirty) { any = true; if (!(await saveOne(rel))) failed = true; }
    paintTabs(); paintTree();
    // A save that failed is not built over: the status keeps saying so.
    if (failed) return;
    if (any) term("==> saved");
    else if (thenBuild) { term("==> nothing to save; every file is saved", "m"); toast("Nothing to save"); }
    if (thenBuild && any) build("you saved");
  }

  /* Files the build, the AI or another editor changed on disk come back in:
   * a file you have not touched is reloaded; one you have changed asks. A
   * later save used to overwrite such a change without a word (K-973). */
  let diskBusy = false;
  async function checkDisk() {
    if (!ide || diskBusy) return;
    diskBusy = true;
    try {
      for (const [rel, doc] of ide.docs) {
        if (doc.error || !doc.state) continue;
        let disk;
        try { disk = await call("ide_read", { path: ide.path, rel }); } catch (e) { continue; }
        if (disk === doc.saved) continue;
        // The editor already holds what is on disk, or holds what disk had
        // when a question went unanswered: nothing of the person's is lost.
        if (textOf(doc) === disk || (doc.diskSeen != null && textOf(doc) === doc.diskSeen)) {
          if (textOf(doc) !== disk) replaceText(rel, doc, disk);
          doc.saved = disk; doc.dirty = false; doc.diskSeen = null; dropDraft(rel); paintTabs(); paintTree(); continue;
        }
        if (!doc.dirty) { replaceText(rel, doc, disk); doc.saved = disk; doc.dirty = false; term(`==> ${rel} changed on disk; reloaded it`, "m"); continue; }
        const k = await ask(`${rel.split("/").pop()} changed on disk while you were editing it.`, [["disk", "Use the file on disk"], ["mine", "Keep mine", true]]);
        if (k === "disk") { replaceText(rel, doc, disk); doc.saved = disk; doc.dirty = false; dropDraft(rel); }
        else if (k === "mine") doc.saved = disk;
        // No answer (another question came first): asked again next time.
        else doc.diskSeen = disk;
        paintTabs(); paintTree();
      }
    } finally { diskBusy = false; }
  }
  window.addEventListener("focus", () => { if (ide && !view.classList.contains("hidden")) checkDisk(); });

  /* ---- build, run, pack, ask ------------------------------------------ */
  function busy(on, label) {
    ide.busy = on;
    ["ideRun", "ideBuild", "ideAskGo", "ideRefresh"].forEach((id) => { const b = $(id); if (b) b.disabled = on; });
    $("ideAskBar").classList.toggle("busy", on);
    if (on) setStatus("busy", label || "Working…");
    // A save that came in while it was busy is built now (K-973: it was
    // dropped without a word).
    else if (ide.queued) { const why = ide.queued; ide.queued = null; setTimeout(() => build(why), 0); }
  }
  async function build(why) {
    if (!ide) return;
    if (ide.busy) { ide.queued = why || "you saved"; term("==> saved; it builds when this finishes", "m"); return; }
    busy(true, "Building…");
    term(`==> ${why ? why + ": " : ""}building ${ide.name}`);
    try {
      const r = await call("ide_build", { path: ide.path });
      busy(false);
      if (r.ok) {
        ide.built = r;
        const secs = (r.millis / 1000).toFixed(1);
        setStatus("ok", `Builds${r.size_bytes ? " · " + kb(r.size_bytes) : ""}`);
        term(`==> built in ${secs} s${r.size_bytes ? " · " + kb(r.size_bytes) : ""}${r.shot ? " · preview is live" : ""}`, "g");
        // What the engine noticed on a good build (usability notes, files it
        // rewrote) is shown, not dropped (K-973).
        const notes = String(r.message || "").split("\n").filter((l) => /^note: /.test(l)).map((l) => l.slice(6));
        notes.forEach((n) => term(`note: ${n}`, "m"));
        problems([], notes);
        checkDisk();
        if (r.shot) showShot(r.shot);
        panelTab("term");
        if (runSave() && why && (why === "you saved" || / changed$/.test(why))) setTimeout(() => runApp("you saved"), 0);
      } else {
        ide.built = null;
        setStatus("bad", "Does not build");
        term(`==> ${r.stage}: ${r.message.split("\n")[0]}`, "r");
        problems([{ stage: r.stage, message: r.message }]);
        $("ideLive").classList.add("hidden");
      }
    } catch (err) {
      busy(false);
      const s = String(err);
      if (!/stopped/.test(s)) ide.built = null;
      setStatus(/stopped/.test(s) ? "idle" : "bad", /stopped/.test(s) ? "Stopped" : "Could not build");
      term(s, "r");
      if (!/stopped/.test(s)) problems([{ stage: "engine", message: s }]);
    }
  }
  function showShot(dataUrl) {
    const img = $("ideShot"), app = $("ideApp");
    img.src = dataUrl; img.classList.remove("hidden");
    $("ideAppNone").classList.add("hidden");
    $("ideLive").classList.remove("hidden");
    app.classList.remove("flash"); void app.offsetWidth; app.classList.add("flash");
    keepShot(ide.path, dataUrl);
  }
  $("ideRefresh").addEventListener("click", () => saveAll(false).then(() => build("reload")));
  $("ideStop").addEventListener("click", () => { if (ide) call("ide_stop", { path: ide.path }).catch(() => {}); });
  async function runApp(why) {
    if (!ide || ide.busy) return;
    await saveAll(false);
    busy(true, "Opening…");
    term(`==> ${why ? why + ": " : ""}packing ${ide.name} and opening it`);
    try {
      const replaced = await call("ide_run", { path: ide.path });
      busy(false);
      setStatus("ok", ide.built && ide.built.size_bytes ? `Builds · ${kb(ide.built.size_bytes)}` : "Opened");
      term(replaced ? "==> closed the copy that was running and opened this one" : "==> opened it in its own window", "g");
    } catch (err) { busy(false); setStatus("bad", "Could not open it"); term(String(err), "r"); problems([{ stage: "run", message: String(err) }]); }
  }
  $("ideRun").addEventListener("click", () => runApp());
  /* Run on save: every save that builds clean opens the new version in
   * place of the one that is running. Remembered on this computer. */
  const RUN_SAVE = "krate-ide-runsave";
  const runSave = () => { try { return localStorage.getItem(RUN_SAVE) === "1"; } catch (e) { return false; } };
  function paintRunSave() {
    const b = $("ideRunSave"), on = runSave();
    b.setAttribute("aria-pressed", on ? "true" : "false");
    b.title = on ? "Runs again every time you save · click to stop" : "Run it again every time you save";
  }
  $("ideRunSave").addEventListener("click", () => {
    const on = !runSave();
    try { localStorage.setItem(RUN_SAVE, on ? "1" : "0"); } catch (e) {}
    paintRunSave();
    if (ide) term(on ? "==> it opens again every time you save" : "==> saving builds it and leaves the window alone", "m");
  });
  paintRunSave();
  $("ideBuild").addEventListener("click", async () => {
    if (!ide || ide.busy) return;
    await saveAll(false);
    busy(true, "Packing…");
    term(`==> packing ${ide.name}.krate`);
    try {
      const r = await call("ide_pack", { path: ide.path });
      busy(false); setStatus("ok", `Builds · ${kb(r.size_bytes)}`);
      term(`==> ${short(r.krate)} · ${kb(r.size_bytes)}`, "g");
      const pre = $("ideTerm");
      const b = document.createElement("button"); b.type = "button"; b.className = "ip-act"; b.textContent = "Show it in its folder";
      b.addEventListener("click", () => call("reveal", { path: r.krate }).catch(() => {}));
      pre.appendChild(b); pre.appendChild(document.createTextNode("\n"));
      panelTab("term"); pre.scrollTop = pre.scrollHeight;
      toast(`Built ${r.krate.split(/[\\/]/).pop()}`);
    } catch (err) { busy(false); setStatus("bad", "Could not pack it"); term(String(err), "r"); problems([{ stage: "pack", message: String(err) }]); }
  });
  const askIn = $("ideAskIn");
  const askPh = askIn.placeholder;
  $("ideAsk").addEventListener("click", () => askIn.focus());
  /* A line diff, small and exact enough to review a change by: the longest
   * common run of lines, then the hunks with three lines either side. */
  function diffLines(a, b) {
    const A0 = a.split("\n"), B0 = b.split("\n");
    // The lines both start and end with are the same; only the middle is
    // compared, so a one-line change in a 3,000-line file is one line.
    let pre = 0; while (pre < A0.length && pre < B0.length && A0[pre] === B0[pre]) pre++;
    let suf = 0; while (suf < A0.length - pre && suf < B0.length - pre && A0[A0.length - 1 - suf] === B0[B0.length - 1 - suf]) suf++;
    const head = A0.slice(0, pre).map((l, k) => [" ", l, k + 1, k + 1]);
    const tail = A0.slice(A0.length - suf).map((l, k) => [" ", l, A0.length - suf + k + 1, B0.length - suf + k + 1]);
    const A = A0.slice(pre, A0.length - suf), B = B0.slice(pre, B0.length - suf);
    const shift = (o) => [o[0], o[1], o[2] ? o[2] + pre : 0, o[3] ? o[3] + pre : 0];
    return head.concat(diffMiddle(A, B).map(shift), tail);
  }
  // Myers' O(ND) diff: fast when the change is small, whatever the size
  // of the file. A change too large to trace is shown as a replacement.
  function diffMiddle(A, B) {
    const n = A.length, m = B.length, max = n + m, off = max;
    if (!n) return B.map((l, j) => ["+", l, 0, j + 1]);
    if (!m) return A.map((l, i) => ["-", l, i + 1, 0]);
    const v = new Int32Array(2 * max + 2), trace = [];
    let found = -1;
    for (let d = 0; d <= Math.min(max, 3000); d++) {
      trace.push(v.slice(off - d, off + d + 2));
      for (let k = -d; k <= d; k += 2) {
        let x = k === -d || (k !== d && v[off + k - 1] < v[off + k + 1]) ? v[off + k + 1] : v[off + k - 1] + 1;
        let y = x - k;
        while (x < n && y < m && A[x] === B[y]) { x++; y++; }
        v[off + k] = x;
        if (x >= n && y >= m) { found = d; break; }
      }
      if (found >= 0) break;
    }
    if (found < 0) return A.map((l, i) => ["-", l, i + 1, 0]).concat(B.map((l, j) => ["+", l, 0, j + 1]));
    const ops = []; let x = n, y = m;
    for (let d = found; d > 0; d--) {
      const t = trace[d], at = (k) => t[k + d];
      const k = x - y;
      const prevK = k === -d || (k !== d && at(k - 1) < at(k + 1)) ? k + 1 : k - 1;
      const px = at(prevK), py = px - prevK;
      while (x > px && y > py) { x--; y--; ops.push([" ", A[x], x + 1, y + 1]); }
      if (x === px) { y--; ops.push(["+", B[y], 0, y + 1]); } else { x--; ops.push(["-", A[x], x + 1, 0]); }
    }
    while (x > 0 && y > 0) { x--; y--; ops.push([" ", A[x], x + 1, y + 1]); }
    return ops.reverse();
  }
  function hunksHtml(ops) {
    const show = new Set();
    ops.forEach((o, k) => { if (o[0] !== " ") for (let d = -3; d <= 3; d++) show.add(k + d); });
    let out = "", last = -2;
    ops.forEach((o, k) => {
      if (!show.has(k)) return;
      if (k !== last + 1) out += `<div class="rv-gap">${o[2] || o[3] ? `line ${o[3] || o[2]}` : ""}</div>`;
      out += `<div class="rv-l ${o[0] === "+" ? "add" : o[0] === "-" ? "del" : ""}"><i>${o[0] === " " ? "" : o[0] === "+" ? "+" : "−"}</i><code>${esc(o[1]) || " "}</code></div>`;
      last = k;
    });
    return out;
  }
  /* The AI's change, shown before it lands: each file as a diff, each one
   * accepted or not. Only what is accepted is written (ide_apply), and the
   * editor's own undo still takes it back afterwards. */
  function review(files) {
    return new Promise((resolve) => {
      const box = document.createElement("div");
      box.className = "ide-review"; box.setAttribute("role", "dialog"); box.setAttribute("aria-label", "Review the change");
      const items = files.map((f, k) => {
        const ops = f.after != null ? diffLines(f.before || "", f.after) : [];
        const add = ops.filter((o) => o[0] === "+").length, del = ops.filter((o) => o[0] === "-").length;
        return `<section class="rv-f" data-k="${k}"><label class="rv-h"><input type="checkbox" checked data-k="${k}"><b>${esc(f.rel)}</b>${f.before == null ? '<span class="rv-new">new</span>' : ""}<span class="rv-n"><span class="a">+${add}</span> <span class="d">−${del}</span></span></label>${f.after != null ? `<div class="rv-d">${hunksHtml(ops)}</div>` : '<p class="rv-bin">Not text; it is replaced whole.</p>'}</section>`;
      }).join("");
      box.innerHTML = `<div class="rv-top"><b>${esc(agentWords())} changed ${files.length} file${files.length === 1 ? "" : "s"}</b><span class="grow"></span><button type="button" class="rv-no">Reject</button><button type="button" class="rv-yes pri">Accept</button></div><div class="rv-body">${items}</div>`;
      $("ideEd").appendChild(box);
      // While the change is reviewed the code stays as it is: typing into
      // it was lost when the change was accepted (K-986).
      const hold = (e) => { if (!box.contains(e.target) && e.target.closest && e.target.closest("#ideCode")) { e.preventDefault(); e.stopPropagation(); } };
      const keys = (e) => {
        if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); done(false); return; }
        hold(e);
      };
      ["beforeinput", "paste", "drop", "cut"].forEach((t) => document.addEventListener(t, hold, true));
      document.addEventListener("keydown", keys, true);
      const done = (ok) => {
        ["beforeinput", "paste", "drop", "cut"].forEach((t) => document.removeEventListener(t, hold, true));
        document.removeEventListener("keydown", keys, true);
        const keep = ok ? [...box.querySelectorAll("input[data-k]")].filter((c) => c.checked).map((c) => files[+c.dataset.k]) : [];
        box.remove(); resolve(keep);
      };
      box.tabIndex = -1;
      requestAnimationFrame(() => { const y = box.querySelector(".rv-yes"); (y || box).focus(); });
      box.querySelector(".rv-yes").addEventListener("click", () => done(true));
      box.querySelector(".rv-no").addEventListener("click", () => done(false));
      box.addEventListener("change", () => {
        const n = [...box.querySelectorAll("input[data-k]")].filter((c) => c.checked).length;
        box.querySelector(".rv-yes").textContent = n === files.length ? "Accept" : n ? `Accept ${n}` : "Accept none";
      });
    });
  }
  // The file and lines the person is looking at go with the request, so
  // "make this faster" means this.
  function askContext() {
    if (!edView || !ide.cur) return "";
    const st = edView.state, sel = st.selection.main;
    if (sel.empty) return `\n\n(I am looking at ${ide.cur}, around line ${st.doc.lineAt(sel.head).number}.)`;
    const a = st.doc.lineAt(sel.from).number, z = st.doc.lineAt(sel.to).number;
    const text = st.sliceDoc(sel.from, sel.to).slice(0, 6000);
    return `\n\n(This is about ${ide.cur}, lines ${a}–${z}:\n\`\`\`\n${text}\n\`\`\`)`;
  }
  async function askAI(request, opts = {}) {
    if (!ide || ide.busy) return;
    await saveAll(false);
    const before = new Map([...ide.docs].map(([rel, d]) => [rel, textOf(d)]));
    const typed = askIn.value;
    askIn.value = ""; askIn.placeholder = `${agentWords()} is working on it…`;
    busy(true, `${agentWords()} is working…`);
    term(`==> asking ${agentWords()}: ${request.split("\n")[0]}`);
    try {
      const r = await call("ide_ask", { path: ide.path, request: request + (opts.noContext ? "" : askContext()), agent: agentName() });
      const files = (r && r.files) || [];
      const settled = () => setStatus(ide.built ? "ok" : ide.lastFail ? "bad" : "idle", ide.built ? "Builds" : ide.lastFail ? "Does not build" : "Not built yet");
      if (!files.length) { busy(false); term("==> nothing needed changing", "g"); settled(); return; }
      term(`==> ${files.length} file${files.length === 1 ? "" : "s"} to review`, "m");
      // Still busy while it is reviewed: Run, Build and a second Ask wait.
      setStatus("busy", "Review the change");
      const keep = await review(files);
      busy(false);
      if (!keep.length) { term("==> the change was not applied; nothing was written", "m"); settled(); return; }
      // A file that changed in the editor since the request (anything that
      // got past the review's hold): the person chooses, nothing is lost.
      const touched = keep.filter((f) => ide.docs.get(f.rel) && ide.docs.get(f.rel).state && before.has(f.rel) && textOf(ide.docs.get(f.rel)) !== before.get(f.rel));
      if (touched.length) {
        setStatus("idle", "Waiting for your answer");
        const k = await ask(`You edited ${touched.map((f) => f.rel.split("/").pop()).join(", ")} while the AI worked. Use the AI's version, or keep your edits?`, [["ai", "Use the AI's"], ["mine", "Keep my edits", true]]);
        if (k !== "ai") {
          const skip = new Set(touched.map((f) => f.rel));
          keep.splice(0, keep.length, ...keep.filter((f) => !skip.has(f.rel)));
          term(`==> kept your edits to ${[...skip].join(", ")}`, "m");
          if (!keep.length) { setStatus(ide.built ? "ok" : "idle", ide.built ? "Builds" : "Not built yet"); return; }
        }
      }
      const changed = await call("ide_apply", { path: ide.path, files: keep.map((f) => ({ rel: f.rel, text: f.after, b64: f.b64 })) });
      term(`==> changed ${changed.join(", ")}`, "g");
      await loadTree();
      for (const rel of changed) {
        const f = keep.find((x) => x.rel === rel); if (!f || f.after == null) continue;
        const text = f.after, old = before.get(rel);
        let doc = ide.docs.get(rel);
        if (!doc || !doc.state) { doc = { saved: text, dirty: false, error: null, state: newState(rel, text) }; ide.docs.set(rel, doc); }
        else { replaceText(rel, doc, text); doc.saved = text; doc.dirty = false; doc.error = null; dropDraft(rel); }
       
        if (old != null) {
          const had = new Set(old.split("\n").map((l) => l.trim()));
          const adds = text.split("\n").map((l, i) => (l.trim() && !had.has(l.trim()) ? i : -1)).filter((i) => i >= 0);
          applyTo(doc, { effects: setAdds.of(adds) });
        }
        if (!ide.open.includes(rel)) ide.open.push(rel);
      }
      if (changed.length && !changed.includes(ide.cur)) ide.cur = changed.find((c) => /\.rs$/.test(c)) || changed[0];
      showDoc(); paintTabs(); paintTree();
      build(`${changed[0]} changed`);
    } catch (err) {
      busy(false);
      setStatus("bad", "The change did not land");
      term(String(err), "r");
      problems([{ stage: "ask", message: String(err) }]);
      // The words typed come back, to send again or change.
      if (!opts.noContext && !askIn.value) askIn.value = typed || request;
    } finally {
      askIn.placeholder = askPh;
    }
  }
  /* Explain: a question about the code, answered in the panel. Nothing in
   * the project changes, so it runs beside a build. The AI is handed the
   * code itself: the selection, or else the open file. */
  function explainContext() {
    if (!edView || !ide.cur) return "";
    const st = edView.state, sel = st.selection.main;
    if (!sel.empty) {
      const a = st.doc.lineAt(sel.from).number, z = st.doc.lineAt(sel.to).number;
      return `\n\nThe code, ${ide.cur} lines ${a}–${z}:\n\`\`\`\n${st.sliceDoc(sel.from, sel.to).slice(0, 12000)}\n\`\`\``;
    }
    const files = ide.tree.filter((e) => !e.dir).map((e) => e.rel).join(", ");
    return `\n\nThe project's files: ${files}\n\nThe open file, ${ide.cur} (the cursor is on line ${st.doc.lineAt(sel.head).number}):\n\`\`\`\n${st.doc.toString().slice(0, 30000)}\n\`\`\``;
  }
  // Plain words with `code` and fenced blocks; everything else escaped.
  function answerHtml(text) {
    return String(text).split(/```[a-z]*\n?/).map((part, i) => i % 2
      ? `<pre>${esc(part.replace(/\n$/, ""))}</pre>`
      : part.split(/\n{2,}/).map((p) => p.trim()).filter(Boolean)
        .map((p) => `<p>${esc(p).replace(/`([^`\n]+)`/g, "<code>$1</code>").replace(/\n/g, "<br>")}</p>`).join("")).join("");
  }
  async function explain(question) {
    if (!ide || ide.explaining) return;
    const sel = edView && !edView.state.selection.main.empty;
    const q = (question || "").trim() || (sel ? "Explain what this code does." : `Explain what ${ide.cur || "this app"} does.`);
    const path = ide.path, out = $("ideAns");
    ide.explaining = true; $("ideExplain").disabled = true;
    askIn.value = "";
    $("ideAnsTab").classList.remove("hidden"); panelTab("ans");
    out.innerHTML = `<div class="ia-q">${esc(q)}</div><div class="ia-wait"><i></i><i></i><i></i><span>${esc(agentWords())} is reading the code…</span></div>`;
    try {
      const a = await call("ide_explain", { path, question: q + explainContext(), agent: agentName() });
      if (!ide || ide.path !== path) return;
      out.innerHTML = `<div class="ia-q">${esc(q)}</div><div class="ia-a">${answerHtml(a || "No answer came back.")}</div><div class="ia-foot">Nothing in the project was changed.</div>`;
    } catch (err) {
      if (!ide || ide.path !== path) return;
      out.innerHTML = `<div class="ia-q">${esc(q)}</div><div class="ia-bad">${esc(String(err))}</div>`;
    } finally {
      if (ide) ide.explaining = false;
      $("ideExplain").disabled = false;
    }
  }
  $("ideExplain").addEventListener("click", () => explain(askIn.value));
  $("ideAskBar").addEventListener("submit", (e) => {
    e.preventDefault();
    const request = askIn.value.trim();
    if (!request) { askIn.focus(); return; }
    askAI(request);
  });
  // Fix with AI: the error, where it is, and what the compiler said.
  $("ideProbs").addEventListener("click", (e) => {
    const b = e.target.closest("[data-fix]"); if (!b || !ide) return;
    e.stopPropagation();
    const d = ide.diags && ide.diags[+b.dataset.fix];
    const req = d ? `Fix this build error in ${d.file} at line ${d.line}: ${d.msg}\n\nWhat the compiler said:\n${d.text}`
      : `Fix why the app does not build. The build said:\n${(ide.lastFail || "").slice(0, 4000)}`;
    askAI(req, { noContext: true });
  }, true);

  $("ipTog").addEventListener("click", () => $("ideMain").classList.toggle("pmin"));
  view.querySelectorAll(".ip-tabs button[data-ip]").forEach((b) => b.addEventListener("click", () => panelTab(b.dataset.ip)));
  document.addEventListener("keydown", (e) => {
    if (!ide || view.classList.contains("hidden")) return;
    const mod = isMac ? e.metaKey : e.ctrlKey;
    if (mod && !e.shiftKey && e.key.toLowerCase() === "i") { e.preventDefault(); askIn.focus(); }
    if (mod && e.shiftKey && e.key.toLowerCase() === "f") { e.preventDefault(); panelTab("find"); }
    if (mod && !e.shiftKey && e.key.toLowerCase() === "p") { e.preventDefault(); quickOpen(); }
    if (mod && !e.shiftKey && e.key.toLowerCase() === "r") { e.preventDefault(); $("ideRun").click(); }
    if (mod && e.shiftKey && e.key.toLowerCase() === "b") { e.preventDefault(); $("ideBuild").click(); }
    if (mod && e.shiftKey && e.key.toLowerCase() === "e") { e.preventDefault(); explain(askIn.value); }
    if (mod && e.key.toLowerCase() === "s" && !(edView && edView.hasFocus)) { e.preventDefault(); saveAll(true); }
  });
  $("ideAskKey").textContent = MOD + "I";
  // The shortcuts, where the buttons are.
  [["ideRun", `Run it (${MOD}R)`], ["ideBuild", `Build the .krate (${MOD}⇧B)`], ["ideExplain", `Explain it, change nothing (${MOD}⇧E)`]].forEach(([id, t]) => { const b = $(id); if (b) b.title = t; });

  /* ---- opening and leaving a project ------------------------------------ */
  async function openProject(p) {
    if (!p || !p.path) return;
    // Every door names a project by the same canonical path, so its drafts,
    // picture and jobs are found whichever way it was opened (K-985).
    try { const r = await call("ide_resolve", { path: p.path }); if (r && r.path) p = { path: r.path, name: p.name || r.name }; } catch (e) { /* the tree says why */ }
    // The project already open: back to it as it is, unsaved edits and all.
    if (ide && ide.path === p.path) {
      try { showView("ide"); } catch (e) {}
      loadSdk().then(paintApi);
      requestAnimationFrame(() => { try { if (edView) edView.focus(); } catch (e) {} });
      return;
    }
    if (ide) await saveAll(false);
    if (ide) for (const d of ide.docs.values()) if (d.view) { d.view.destroy(); d.host.remove(); }
    edView = null;
    ide = { path: p.path, name: p.name || p.path.split(/[\\/]/).pop(), tree: [], docs: new Map(), open: [], cur: null, busy: false, built: null, collapsed: new Set(), queued: null };
    $("ideName").textContent = ide.name;
    $("idePath").textContent = short(ide.path);
    $("ideFrameName").textContent = ide.name.replace(/[-_]+/g, " ").replace(/\b\w/g, (c) => c.toUpperCase());
    $("ideTerm").textContent = "";
    $("ideShot").classList.add("hidden"); $("ideShot").removeAttribute("src");
    $("ideAppNone").classList.remove("hidden"); $("ideLive").classList.add("hidden");
    const cached = shots()[ide.path];
    if (cached) { $("ideShot").src = cached.src; $("ideShot").classList.remove("hidden"); $("ideAppNone").classList.add("hidden"); }
    $("ideAns").textContent = ""; $("ideAnsTab").classList.add("hidden");
    problems([]); panelTab("term");
    setStatus("idle", "Not built yet");
    paintAgent();
    try { showView("ide"); } catch (e) { /* app.js not loaded */ }
    loadSdk().then(paintApi);
    term(`~ ${short(ide.path)}`, "m");
    await loadTree();
    const first = ["src/lib.rs", "src/main.rs"].find((r) => ide.tree.some((e) => e.rel === r)) || (ide.tree.find((e) => !e.dir && /\.rs$/.test(e.rel)) || {}).rel;
    if (first) await openFile(first);
    if (ide.tree.some((e) => e.rel === "manifest.toml")) { ide.open.push("manifest.toml"); paintTabs(); }
    build("opened");
  }
  function paintAgent() {
    const box = $("ideAskAgent");
    try { box.innerHTML = aiLogo(agentName()); } catch (e) { box.textContent = ""; }
    box.title = `${agentWords()} makes the change`;
  }
  $("ideBack").addEventListener("click", async () => {
    await saveAll(false);
    const tab = $("homeIdeTab");
    const home = document.querySelector('#side .side-row[data-side="home"]');
    if (home) home.click();
    // Through Home's own mode switch, so its hint, search and the lit
    // sidebar row are reset (a click on a tab already on did nothing).
    setTimeout(() => { if (window.krHomeMode) window.krHomeMode("ide"); else if (tab) tab.click(); }, 50);
  });

  /* ---- the doors: the sidebar row and Home's IDE tab ------------------ */
  const sideRow = $("sideIde");
  if (sideRow) sideRow.addEventListener("click", () => {
    if (ide) { try { showView("ide"); } catch (e) {} requestAnimationFrame(() => { try { if (edView) edView.focus(); } catch (e) {} }); return; }
    const home = document.querySelector('#side .side-row[data-side="home"]');
    if (home && $("viewHome").classList.contains("hidden")) home.click();
    setTimeout(() => { if (window.krHomeMode) window.krHomeMode("ide"); else { const t = $("homeIdeTab"); if (t) t.click(); } }, 50);
  });

  let projects = [], projectsAsked = 0;
  async function loadProjects() {
    // Only the newest answer is drawn: a slow first one does not paint over it.
    const mine = ++projectsAsked;
    let list = [], why = "";
    try { list = (await call("ide_projects")) || []; } catch (e) { why = String(e); }
    if (mine !== projectsAsked) return;
    projects = (Array.isArray(list) ? list : []).filter((p) => p && typeof p.path === "string" && p.path).map((p) => ({ ...p, name: typeof p.name === "string" && p.name ? p.name : p.path.split(/[\\/]/).filter(Boolean).pop() }));
    if (why) hint(`Could not list your projects: ${why}`);
    paintProjects();
  }
  function paintProjects() {
    const box = $("homeIdeRows"); if (!box) return;
    const q = ($("homeIdeFind").value || "").trim().toLowerCase();
    const list = projects.filter((p) => !q || p.name.toLowerCase().includes(q) || p.path.toLowerCase().includes(q));
    const pics = shots();
    // Seven at a glance; a search shows every match.
    const shown = q ? list.slice(0, 40) : list.slice(0, 7);
    box.innerHTML = shown.map((p, i) => `<button type="button" class="kr-start" style="--k:${i}" data-path="${esc(p.path)}" data-name="${esc(p.name)}"><span class="th">${pics[p.path] ? `<img src="${esc(pics[p.path].src)}" alt="">` : `<span class="th-none">${ICON.rs}</span>`}</span><b>${esc(p.name)}</b><small>${esc(short(p.path))}${p.updated ? " · " + esc(ago(p.updated)) : ""}</small></button>`).join("") +
      (q && !list.length ? `<p class="kr-ide-none">No project called “${esc(q)}”. Press Enter to open it as a path, or start a new one.</p>` : "") +
      (!q && list.length > 7 ? `<p class="kr-ide-more">${list.length - 7} more · type a name to find one</p>` : "") +
      (q ? "" : `<button type="button" class="kr-start newp" style="--k:${Math.min(list.length, 7)}" data-new="1"><span class="th"><svg width="18" height="18" viewBox="0 0 16 16" fill="none"><path d="M8 3.4v9.2M3.4 8h9.2" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/></svg></span><b>New project</b><small>A working starter to build on</small></button>`);
  }
  $("homeIdeRows").addEventListener("click", (e) => {
    const b = e.target.closest(".kr-start"); if (!b) return;
    if (b.dataset.new) { newProject(); return; }
    openProject({ path: b.dataset.path, name: b.dataset.name });
  });
  $("homeIdeFind").addEventListener("input", paintProjects);
  async function openTyped() {
    const v = $("homeIdeFind").value.trim();
    if (!v) { const first = projects[0]; if (first) openProject(first); return; }
    const hit = projects.find((p) => p.name.toLowerCase() === v.toLowerCase());
    if (hit) { openProject(hit); return; }
    if (/^[~/\\]|^[A-Za-z]:\\/.test(v)) {
      // Resolved to the one path every door uses (a typed ~/ path was filed
      // under its spelling, so its drafts and picture went missing).
      try { openProject(await call("ide_resolve", { path: v })); }
      catch (err) { hint(String(err)); }
      return;
    }
    const near = projects.find((p) => p.name.toLowerCase().includes(v.toLowerCase()));
    if (near) openProject(near); else hint("No project by that name. Open its folder, or start a new one.");
  }
  $("homeIdeFind").addEventListener("keydown", (e) => { if (e.key === "Enter") { e.preventDefault(); openTyped(); } });
  $("homeIdeGo").addEventListener("click", openTyped);
  $("homeIdeOpen").addEventListener("click", async () => {
    try { const p = await call("ide_open_folder"); if (p) openProject(p); }
    catch (err) { hint(String(err)); }
  });
  $("homeIdeNew").addEventListener("click", newProject);
  function hint(text) { const h = $("homeIdeHint"); if (h) { h.textContent = text; h.hidden = !text; } }

  // A new project: a name, then a working starter made by the engine's own
  // template, opened straight away.
  function newProject() {
    const pane = $("homeIdePane");
    if (pane.querySelector(".kr-newp")) { pane.querySelector(".kr-newp input").focus(); return; }
    const f = document.createElement("form");
    f.className = "kr-newp"; f.autocomplete = "off";
    // Which of the engine's own starters to begin from.
    f.innerHTML = '<input type="text" maxlength="40" placeholder="Name your project, like habit tracker" aria-label="Project name"><button type="submit" class="kr-mk">Make it</button><button type="button" class="kr-cx">Cancel</button>' +
      '<div class="kr-np-kinds" role="radiogroup" aria-label="Start from">' +
      [["checklist", "A window app", "a checklist that saves"], ["word-frequency", "A command-line tool", "reads a file, prints words"], ["voice-prompter", "A voice app", "listens to the microphone"]]
        .map(([k, t, d], i) => `<label><input type="radio" name="kind" value="${k}"${i ? "" : " checked"}><b>${t}</b><small>${d}</small></label>`).join("") +
      '</div><p class="kr-np-line" aria-live="polite"></p>';
    pane.appendChild(f);
    const input = f.querySelector("input"), line = f.querySelector(".kr-np-line");
    input.focus();
    f.querySelector(".kr-cx").addEventListener("click", () => f.remove());
    f.addEventListener("keydown", (e) => { if (e.key === "Escape" && !f.classList.contains("busy")) { e.preventDefault(); f.remove(); } });
    let off = null;
    f.addEventListener("submit", async (e) => {
      e.preventDefault();
      const name = input.value.trim();
      if (!name) { line.textContent = "Give it a name first."; input.value = ""; input.focus(); return; }
      f.classList.add("busy"); f.querySelectorAll("button, input").forEach((x) => (x.disabled = true));
      line.textContent = "Making a working starter…";
      try { off = await tauri.event.listen("ide-line", (ev) => { const l = ev.payload && ev.payload.line; if (l && l.trim()) line.textContent = l.trim().slice(0, 140); }); } catch (err) { off = null; }
      try {
        const kind = (f.querySelector('input[name="kind"]:checked') || {}).value || "checklist";
        const p = await call("ide_new", { name, kind });
        f.remove();
        await loadProjects();
        openProject(p);
      } catch (err) {
        f.classList.remove("busy"); f.querySelectorAll("button, input").forEach((x) => (x.disabled = false));
        line.textContent = String(err);
        input.focus(); input.select();
      } finally { if (off) try { off(); } catch (err) {} }
    });
  }

  // Home tells us when its IDE tab is chosen (redesign.js setMode).
  // Each visit starts clean: no old hint, no old search.
  document.addEventListener("kr-home-mode", (e) => { if (e.detail === "ide") { hint(""); $("homeIdeFind").value = ""; loadProjects(); paintAgent(); } });
  // Closing the window: anything not saved is already kept as a draft, and
  // comes back the next time the project opens.
  window.addEventListener("beforeunload", () => { if (ide) saveAll(false); });
  // Studio's Code pane opens a built app's own source here.
  window.krIdeOpen = (path, name) => openProject({ path, name });
})();
