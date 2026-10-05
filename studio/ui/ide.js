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
  function extensionsFor(rel) {
    const K = CM; cmParts();
    const lang = /\.rs$/.test(rel) ? K.rust() : /\.toml$/.test(rel) ? K.toml : [];
    return [
      K.lintGutter(), K.lineNumbers(), K.highlightActiveLineGutter(), K.highlightSpecialChars(), K.history(),
      K.foldGutter({ openText: "▾", closedText: "▸" }), K.drawSelection(), K.dropCursor(),
      K.EditorState.allowMultipleSelections.of(true), K.indentOnInput(), K.syntaxHighlighting(hlStyle),
      K.bracketMatching(), K.closeBrackets(), K.autocompletion(), K.rectangularSelection(), K.crosshairCursor(),
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
      K.EditorView.updateListener.of((u) => onEditorUpdate(rel, u)),
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
    const d = ide.diags[+b.dataset.d]; if (d) goTo(d.file, d.line, d.col);
  });
  async function goTo(rel, line, col) {
    if (!ide.tree.some((t) => t.rel === rel)) { term(`${rel} is not in this project`, "r"); return; }
    if (ide.cur !== rel || !edView) await openFile(rel);
    if (!edView) return;
    const doc = edView.state.doc;
    const ln = doc.line(Math.max(1, Math.min(line || 1, doc.lines)));
    const at = Math.min(ln.from + Math.max(0, (col || 1) - 1), ln.to);
    edView.dispatch({ selection: { anchor: at }, scrollIntoView: true });
    edView.focus();
  }
  // The errors drawn in the editor too: a mark in the gutter and a line
  // under the code, for every open file the compiler named.
  function markDiags() {
    if (!CM || !ide) return;
    for (const [rel, doc] of ide.docs) {
      if (!doc.state) continue;
      const mine = (ide.diags || []).filter((d) => d.file === rel);
      const list = mine.map((d) => {
        const n = doc.state.doc;
        const ln = n.line(Math.max(1, Math.min(d.line, n.lines)));
        const from = Math.min(ln.from + Math.max(0, d.col - 1), ln.to);
        const word = n.sliceString(from, ln.to).match(/^[\w:.]+/);
        return { from, to: Math.min(ln.to, from + Math.max(1, word ? word[0].length : 1)), severity: d.sev === "warning" ? "warning" : "error", message: d.msg + (d.code ? ` (${d.code})` : "") };
      });
      const spec = CM.setDiagnostics(doc.state, list);
      if (edView && ide.cur === rel) edView.dispatch(spec);
      else doc.state = doc.state.update(spec).state;
    }
  }
  function panelTab(which) {
    view.querySelectorAll(".ip-tabs button[data-ip]").forEach((b) => b.classList.toggle("on", b.dataset.ip === which));
    $("ideTerm").classList.toggle("hidden", which !== "term");
    $("ideProbs").classList.toggle("hidden", which !== "prob");
    $("ideFind").classList.toggle("hidden", which !== "find");
    $("ideMain").classList.remove("pmin");
    if (which === "find") setTimeout(() => $("ideFindIn").focus(), 30);
  }

  /* Search every file in the project (Cmd-Shift-F). Text files are read
   * once and kept; an open file is searched as it is now, unsaved edits
   * and all. */
  const findCache = new Map();
  let findT = 0;
  async function findAll(q) {
    const out = $("ideFindOut");
    if (!ide || !q) { out.innerHTML = ""; return; }
    const files = ide.tree.filter((e) => !e.dir && /\.(rs|toml|md|txt|json|wit)$/i.test(e.rel) && (e.size || 0) < 400000);
    const needle = q.toLowerCase(); const hits = [];
    for (const f of files) {
      let text = ide.docs.get(f.rel) && ide.docs.get(f.rel).state ? textOf(ide.docs.get(f.rel)) : findCache.get(f.rel);
      if (text == null) { try { text = await call("ide_read", { path: ide.path, rel: f.rel }); findCache.set(f.rel, text); } catch (e) { continue; } }
      text.split("\n").forEach((l, i) => { const at = l.toLowerCase().indexOf(needle); if (at >= 0 && hits.length < 300) hits.push({ rel: f.rel, line: i + 1, col: at + 1, l }); });
    }
    if ($("ideFindIn").value.trim() !== q) return;
    out.innerHTML = hits.length ? hits.map((h, i) => {
      const a = Math.max(0, h.col - 41), s = h.l.slice(a, h.col - 1), m = h.l.slice(h.col - 1, h.col - 1 + q.length), r = h.l.slice(h.col - 1 + q.length, h.col + 80);
      return `<button type="button" class="ip-hit" data-h="${i}"><b>${esc(h.rel)}:${h.line}</b><span>${a ? "…" : ""}${esc(s.trimStart())}<mark>${esc(m)}</mark>${esc(r)}</span></button>`;
    }).join("") + (hits.length >= 300 ? '<p class="ip-ok">The first 300 matches.</p>' : "") : '<p class="ip-ok">Nothing matches.</p>';
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
    try { ide.tree = (await call("ide_tree", { path: ide.path })) || []; } catch (e) { ide.tree = []; term(String(e), "r"); }
    paintTree();
  }
  function paintTree() {
    const box = $("ideTree");
    const hiddenUnder = [...ide.collapsed];
    box.innerHTML = `<div class="th2">${esc(ide.name.toUpperCase())}</div>` + ide.tree.map((e) => {
      const depth = e.rel.split("/").length - 1;
      const gone = hiddenUnder.some((d) => e.rel.startsWith(d + "/"));
      if (gone) return "";
      const name = e.rel.split("/").pop();
      const doc = ide.docs.get(e.rel);
      const on = !e.dir && e.rel === ide.cur;
      return `<button type="button" class="tn${on ? " on" : ""}${e.dir ? " dir" : ""}${e.dir && ide.collapsed.has(e.rel) ? " shut" : ""}" style="--lv:${depth}" data-rel="${esc(e.rel)}" data-dir="${e.dir ? 1 : ""}" title="${esc(e.rel)}">${iconFor(e)}<span>${esc(name)}</span>${doc && doc.dirty ? '<i class="md" title="Not saved"></i>' : ""}</button>`;
    }).join("");
  }
  $("ideTree").addEventListener("click", (e) => {
    const b = e.target.closest(".tn"); if (!b || !ide) return;
    const rel = b.dataset.rel;
    if (b.dataset.dir) { ide.collapsed.has(rel) ? ide.collapsed.delete(rel) : ide.collapsed.add(rel); paintTree(); return; }
    openFile(rel);
  });

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
  function newState(rel, text) { return CM.EditorState.create({ doc: text, extensions: extensionsFor(rel) }); }
  // Replace a file's text without losing its undo history.
  function replaceText(rel, doc, text) {
    const len = doc.state.doc.length;
    const tr = { changes: { from: 0, to: len, insert: text } };
    if (edView && ide.cur === rel) edView.dispatch(tr);
    else doc.state = doc.state.update(tr).state;
  }

  /* Unsaved work is kept as a draft as you type, so closing the window, a
   * crash or a reload never loses it; it comes back the next time the file
   * opens, marked as not saved (K-973). */
  const DRAFTS = "krate-ide-drafts";
  const drafts = () => { try { return JSON.parse(localStorage.getItem(DRAFTS) || "{}"); } catch (e) { return {}; } };
  let draftT = 0;
  function keepDraft(rel, doc) {
    clearTimeout(draftT);
    draftT = setTimeout(() => {
      try {
        const all = drafts(); const mine = all[ide.path] || {};
        if (doc.dirty) mine[rel] = textOf(doc); else delete mine[rel];
        if (Object.keys(mine).length) all[ide.path] = mine; else delete all[ide.path];
        localStorage.setItem(DRAFTS, JSON.stringify(all));
      } catch (e) { /* a full or blocked store: the save is still the save */ }
    }, 300);
  }
  function dropDraft(rel) {
    try { const all = drafts(); if (all[ide.path]) { delete all[ide.path][rel]; if (!Object.keys(all[ide.path]).length) delete all[ide.path]; localStorage.setItem(DRAFTS, JSON.stringify(all)); } } catch (e) {}
  }

  /* A question above the editor, answered with one of its buttons. */
  function ask(text, choices) {
    const bar = $("ideBar");
    return new Promise((res) => {
      bar.querySelector(".t").textContent = text;
      bar.querySelectorAll("button").forEach((b) => b.remove());
      for (const [key, label, primary] of choices) {
        const b = document.createElement("button"); b.type = "button"; b.textContent = label;
        if (primary) b.className = "pri";
        b.addEventListener("click", () => { bar.classList.add("hidden"); res(key); });
        bar.appendChild(b);
      }
      bar.classList.remove("hidden");
    });
  }

  async function openFile(rel) {
    if (!ide) return;
    try { await loadCM(); } catch (err) { $("ideMsg").textContent = String(err.message || err); $("ideMsg").classList.remove("hidden"); return; }
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
  function showDoc() {
    const doc = ide && ide.cur ? ide.docs.get(ide.cur) : null;
    const code = $("ideCode"), msg = $("ideMsg"), empty = $("ideEmpty");
    empty.classList.toggle("hidden", !!doc);
    code.classList.toggle("hidden", !doc || !!doc.error);
    msg.classList.toggle("hidden", !doc || !doc.error);
    if (!doc) return;
    if (doc.error) { msg.textContent = `${ide.cur}: ${doc.error}`; return; }
    if (!edView) edView = new CM.EditorView({ state: doc.state, parent: code });
    else edView.setState(doc.state);
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
        if (k === "cancel") return;
        if (k === "save" && !(await saveOne(rel))) return;
        if (k === "drop") { dropDraft(rel); ide.docs.delete(rel); }
      }
      ide.open = ide.open.filter((r) => r !== rel);
      if (ide.cur === rel) ide.cur = ide.open[ide.open.length - 1] || null;
      showDoc(); paintTabs(); paintTree(); return;
    }
    const t = e.target.closest("[data-rel]"); if (t && t.dataset.rel !== ide.cur) openFile(t.dataset.rel);
  });

  async function saveOne(rel) {
    const doc = ide.docs.get(rel); if (!doc || !doc.dirty || doc.error) return true;
    const text = textOf(doc);
    try {
      await call("ide_write", { path: ide.path, rel, text });
      doc.saved = text; doc.dirty = textOf(doc) !== doc.saved; dropDraft(rel); findCache.delete(rel); return true;
    } catch (err) { term(`could not save ${rel}: ${err}`, "r"); return false; }
  }
  async function saveAll(thenBuild) {
    if (!ide) return;
    let any = false;
    for (const [rel, doc] of ide.docs) if (doc.dirty) { any = true; await saveOne(rel); }
    paintTabs(); paintTree();
    if (any) term("==> saved");
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
        if (!doc.dirty) { replaceText(rel, doc, disk); doc.saved = disk; doc.dirty = false; term(`==> ${rel} changed on disk; reloaded it`, "m"); continue; }
        const k = await ask(`${rel.split("/").pop()} changed on disk while you were editing it.`, [["disk", "Use the file on disk"], ["mine", "Keep mine", true]]);
        if (k === "disk") { replaceText(rel, doc, disk); doc.saved = disk; doc.dirty = false; dropDraft(rel); }
        else doc.saved = disk;
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
      } else {
        setStatus("bad", "Does not build");
        term(`==> ${r.stage}: ${r.message.split("\n")[0]}`, "r");
        problems([{ stage: r.stage, message: r.message }]);
        $("ideLive").classList.add("hidden");
      }
    } catch (err) {
      busy(false);
      const s = String(err);
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
  $("ideRun").addEventListener("click", async () => {
    if (!ide || ide.busy) return;
    await saveAll(false);
    busy(true, "Opening…");
    term(`==> packing ${ide.name} and opening it`);
    try { await call("ide_run", { path: ide.path }); busy(false); setStatus("ok", ide.built && ide.built.size_bytes ? `Builds · ${kb(ide.built.size_bytes)}` : "Opened"); term("==> opened it in its own window", "g"); }
    catch (err) { busy(false); setStatus("bad", "Could not open it"); term(String(err), "r"); problems([{ stage: "run", message: String(err) }]); }
  });
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
      pre.appendChild(b); pre.appendChild(document.createTextNode("\n")); pre.scrollTop = pre.scrollHeight;
      toast(`Built ${r.krate.split(/[\\/]/).pop()}`);
    } catch (err) { busy(false); setStatus("bad", "Could not pack it"); term(String(err), "r"); problems([{ stage: "pack", message: String(err) }]); }
  });
  const askIn = $("ideAskIn");
  const askPh = askIn.placeholder;
  $("ideAsk").addEventListener("click", () => askIn.focus());
  /* A line diff, small and exact enough to review a change by: the longest
   * common run of lines, then the hunks with three lines either side. */
  function diffLines(a, b) {
    const A = a.split("\n"), B = b.split("\n");
    if (A.length * B.length > 4e6) return B.map((l) => ["+", l]).concat(A.map((l) => ["-", l]));
    const n = A.length, m = B.length;
    const L = Array.from({ length: n + 1 }, () => new Uint32Array(m + 1));
    for (let i = n - 1; i >= 0; i--) for (let j = m - 1; j >= 0; j--) L[i][j] = A[i] === B[j] ? L[i + 1][j + 1] + 1 : Math.max(L[i + 1][j], L[i][j + 1]);
    const ops = []; let i = 0, j = 0;
    while (i < n && j < m) { if (A[i] === B[j]) { ops.push([" ", A[i], i + 1, j + 1]); i++; j++; } else if (L[i + 1][j] >= L[i][j + 1]) { ops.push(["-", A[i], i + 1, 0]); i++; } else { ops.push(["+", B[j], 0, j + 1]); j++; } }
    while (i < n) { ops.push(["-", A[i], i + 1, 0]); i++; }
    while (j < m) { ops.push(["+", B[j], 0, j + 1]); j++; }
    return ops;
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
      const done = (ok) => {
        const keep = ok ? [...box.querySelectorAll("input[data-k]")].filter((c) => c.checked).map((c) => files[+c.dataset.k]) : [];
        box.remove(); resolve(keep);
      };
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
    askIn.value = ""; askIn.placeholder = `${agentWords()} is working on it…`;
    busy(true, `${agentWords()} is working…`);
    term(`==> asking ${agentWords()}: ${request.split("\n")[0]}`);
    try {
      const r = await call("ide_ask", { path: ide.path, request: request + (opts.noContext ? "" : askContext()), agent: agentName() });
      const files = (r && r.files) || [];
      busy(false);
      if (!files.length) { term("==> nothing needed changing", "g"); setStatus(ide.built ? "ok" : "idle", ide.built ? "Builds" : "Not built yet"); return; }
      term(`==> ${files.length} file${files.length === 1 ? "" : "s"} to review`, "m");
      setStatus("idle", "Review the change");
      const keep = await review(files);
      if (!keep.length) { term("==> the change was not applied; nothing was written", "m"); setStatus(ide.built ? "ok" : "idle", ide.built ? "Builds" : "Not built yet"); return; }
      const changed = await call("ide_apply", { path: ide.path, files: keep.map((f) => ({ rel: f.rel, text: f.after, b64: f.b64 })) });
      term(`==> changed ${changed.join(", ")}`, "g");
      await loadTree();
      for (const rel of changed) {
        const f = keep.find((x) => x.rel === rel); if (!f || f.after == null) continue;
        const text = f.after, old = before.get(rel);
        let doc = ide.docs.get(rel);
        if (!doc || !doc.state) { doc = { saved: text, dirty: false, error: null, state: newState(rel, text) }; ide.docs.set(rel, doc); }
        else { replaceText(rel, doc, text); doc.saved = text; doc.dirty = false; doc.error = null; dropDraft(rel); }
        findCache.delete(rel);
        if (old != null) {
          const had = new Set(old.split("\n").map((l) => l.trim()));
          const adds = text.split("\n").map((l, i) => (l.trim() && !had.has(l.trim()) ? i : -1)).filter((i) => i >= 0);
          doc.state = doc.state.update({ effects: setAdds.of(adds) }).state;
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
    } finally {
      askIn.placeholder = askPh;
    }
  }
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
    if (mod && e.key.toLowerCase() === "s" && !(edView && edView.hasFocus)) { e.preventDefault(); saveAll(true); }
  });
  $("ideAskKey").textContent = MOD + "I";

  /* ---- opening and leaving a project ------------------------------------ */
  async function openProject(p) {
    if (!p || !p.path) return;
    if (ide && ide.path !== p.path) await saveAll(false);
    ide = { path: p.path, name: p.name || p.path.split(/[\\/]/).pop(), tree: [], docs: new Map(), open: [], cur: null, busy: false, built: null, collapsed: new Set(), queued: null };
    if (edView) { edView.destroy(); edView = null; }
    $("ideName").textContent = ide.name;
    $("idePath").textContent = short(ide.path);
    $("ideFrameName").textContent = ide.name.replace(/[-_]+/g, " ").replace(/\b\w/g, (c) => c.toUpperCase());
    $("ideTerm").textContent = "";
    $("ideShot").classList.add("hidden"); $("ideShot").removeAttribute("src");
    $("ideAppNone").classList.remove("hidden"); $("ideLive").classList.add("hidden");
    const cached = shots()[ide.path];
    if (cached) { $("ideShot").src = cached.src; $("ideShot").classList.remove("hidden"); $("ideAppNone").classList.add("hidden"); }
    problems([]); panelTab("term");
    setStatus("idle", "Not built yet");
    paintAgent();
    try { showView("ide"); } catch (e) { /* app.js not loaded */ }
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
    setTimeout(() => { if (tab) tab.click(); }, 50);
  });

  /* ---- the doors: the sidebar row and Home's IDE tab ------------------ */
  const sideRow = $("sideIde");
  if (sideRow) sideRow.addEventListener("click", () => {
    if (ide) { try { showView("ide"); } catch (e) {} return; }
    const home = document.querySelector('#side .side-row[data-side="home"]');
    if (home && $("viewHome").classList.contains("hidden")) home.click();
    setTimeout(() => { const t = $("homeIdeTab"); if (t) t.click(); }, 50);
  });

  let projects = [];
  async function loadProjects() {
    try { projects = (await call("ide_projects")) || []; } catch (e) { projects = []; }
    paintProjects();
  }
  function paintProjects() {
    const box = $("homeIdeRows"); if (!box) return;
    const q = ($("homeIdeFind").value || "").trim().toLowerCase();
    const list = projects.filter((p) => !q || p.name.toLowerCase().includes(q) || p.path.toLowerCase().includes(q));
    const pics = shots();
    box.innerHTML = list.slice(0, 7).map((p, i) => `<button type="button" class="kr-start" style="--k:${i}" data-path="${esc(p.path)}" data-name="${esc(p.name)}"><span class="th">${pics[p.path] ? `<img src="${esc(pics[p.path].src)}" alt="">` : `<span class="th-none">${ICON.rs}</span>`}</span><b>${esc(p.name)}</b><small>${esc(short(p.path))}${p.updated ? " · " + esc(ago(p.updated)) : ""}</small></button>`).join("") +
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
      try { await call("ide_tree", { path: v }); openProject({ path: v, name: v.split(/[\\/]/).filter(Boolean).pop() }); }
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
    f.innerHTML = '<input type="text" maxlength="40" placeholder="Name your project, like habit tracker" aria-label="Project name" required><button type="submit" class="kr-mk">Make it</button><button type="button" class="kr-cx">Cancel</button><p class="kr-np-line" aria-live="polite"></p>';
    pane.appendChild(f);
    const input = f.querySelector("input"), line = f.querySelector(".kr-np-line");
    input.focus();
    f.querySelector(".kr-cx").addEventListener("click", () => f.remove());
    let off = null;
    f.addEventListener("submit", async (e) => {
      e.preventDefault();
      const name = input.value.trim(); if (!name) return;
      f.classList.add("busy"); f.querySelectorAll("button, input").forEach((x) => (x.disabled = true));
      line.textContent = "Making a working starter…";
      try { off = await tauri.event.listen("ide-line", (ev) => { const l = ev.payload && ev.payload.line; if (l && l.trim()) line.textContent = l.trim().slice(0, 140); }); } catch (err) { off = null; }
      try {
        const p = await call("ide_new", { name });
        f.remove();
        await loadProjects();
        openProject(p);
      } catch (err) {
        f.classList.remove("busy"); f.querySelectorAll("button, input").forEach((x) => (x.disabled = false));
        line.textContent = String(err);
      } finally { if (off) try { off(); } catch (err) {} }
    });
  }

  // Home tells us when its IDE tab is chosen (redesign.js setMode).
  document.addEventListener("kr-home-mode", (e) => { if (e.detail === "ide") { loadProjects(); paintAgent(); } });
  // Closing the window: anything not saved is already kept as a draft, and
  // comes back the next time the project opens.
  window.addEventListener("beforeunload", () => { if (ide) saveAll(false); });
  // Studio's Code pane opens a built app's own source here.
  window.krIdeOpen = (path, name) => openProject({ path, name });
})();
