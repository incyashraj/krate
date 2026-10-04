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

  /* ---- highlighting ---------------------------------------------------- */
  const RUST = /(\/\/.*$)|(\/\*.*?\*\/)|("(?:[^"\\]|\\.)*")|('(?:[^'\\]|\\.)')|(#!?\[[^\]]*\])|\b(fn|let|mut|use|const|static|struct|enum|impl|trait|pub|mod|if|else|match|for|while|loop|in|return|extern|crate|as|self|super|where|move|ref|true|false|break|continue|type|unsafe|dyn)\b|\b(\d[\d_]*(?:\.\d+)?(?:[a-z]\w*)?)\b|\b([A-Z][A-Za-z0-9_]*)\b|\b([a-z_][a-z0-9_]*!)|\b([a-z_][a-z0-9_]*)(?=\s*\()/g;
  const TOML = /(#.*$)|(^\s*\[[^\]]*\])|("(?:[^"\\]|\\.)*")|\b(true|false)\b|(^\s*[A-Za-z0-9_.-]+(?=\s*=))|\b(\d[\d_.]*)\b/g;
  function hlLine(line, lang) {
    const rx = lang === "rs" ? RUST : lang === "toml" ? TOML : null;
    if (!rx) return esc(line);
    rx.lastIndex = 0;
    let out = "", at = 0, m;
    while ((m = rx.exec(line))) {
      if (m[0] === "") { rx.lastIndex++; continue; }
      out += esc(line.slice(at, m.index));
      let cls = "";
      if (lang === "rs") cls = m[1] || m[2] ? "c" : m[3] || m[4] ? "s" : m[5] ? "a" : m[6] ? "k" : m[7] ? "n" : m[8] ? "t" : m[9] ? "m" : "f";
      else cls = m[1] ? "c" : m[2] ? "f" : m[3] ? "s" : m[4] ? "k" : m[5] ? "t" : "n";
      out += `<span class="tk-${cls}">${esc(m[0])}</span>`;
      at = m.index + m[0].length;
    }
    return out + esc(line.slice(at));
  }
  const langOf = (rel) => /\.rs$/.test(rel) ? "rs" : /\.toml$/.test(rel) ? "toml" : "";

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
  function problems(list) {
    const box = $("ideProbs");
    $("ideProbN").textContent = String(list.length);
    box.innerHTML = list.length ? list.map((p) => `<div class="ip-prob"><b>${esc(p.stage)}</b><pre>${esc(p.message)}</pre></div>`).join("")
      : '<p class="ip-ok">No problems. It builds, imports only Krate, and runs.</p>';
    if (list.length) panelTab("prob");
  }
  function panelTab(which) {
    view.querySelectorAll(".ip-tabs button[data-ip]").forEach((b) => b.classList.toggle("on", b.dataset.ip === which));
    $("ideTerm").classList.toggle("hidden", which !== "term");
    $("ideProbs").classList.toggle("hidden", which !== "prob");
    $("ideMain").classList.remove("pmin");
  }

  try {
    tauri.event.listen("ide-line", (e) => {
      const p = e.payload || {};
      if (!ide || !p.line) return;
      if (p.path && p.path !== ide.path && !p.path.endsWith("/" + ide.name)) return;
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
  const ed = $("ideEd");
  ed.insertAdjacentHTML("beforeend", '<div class="ide-code hidden" id="ideCode"><div class="ide-gut" aria-hidden="true"><div class="ide-gutin" id="ideGut"></div></div><div class="ide-cw"><pre class="ide-hl" id="ideHl" aria-hidden="true"></pre><textarea class="ide-src" id="ideSrc" spellcheck="false" autocapitalize="off" autocomplete="off" autocorrect="off" wrap="off" aria-label="Code"></textarea></div></div><div class="ide-msg hidden" id="ideMsg"></div>');
  const src = $("ideSrc"), hl = $("ideHl"), gut = $("ideGut");
  let paintQueued = false;
  function paintCode() {
    paintQueued = false;
    const doc = ide && ide.docs.get(ide.cur); if (!doc) return;
    const lines = src.value.split("\n");
    const lang = langOf(ide.cur);
    hl.innerHTML = lines.map((l, i) => `<span class="l${doc.adds && doc.adds.has(i) ? " add" : ""}">${hlLine(l, lang) || "\u200b"}</span>`).join("");
    if (gut.childElementCount !== lines.length) gut.innerHTML = lines.map((_, i) => `<span>${i + 1}</span>`).join("");
    syncScroll();
  }
  const queuePaint = () => { if (!paintQueued) { paintQueued = true; requestAnimationFrame(paintCode); } };
  function syncScroll() {
    hl.style.transform = `translate(${-src.scrollLeft}px, ${-src.scrollTop}px)`;
    gut.style.transform = `translateY(${-src.scrollTop}px)`;
  }
  src.addEventListener("scroll", syncScroll);
  src.addEventListener("input", () => {
    const doc = ide && ide.docs.get(ide.cur); if (!doc) return;
    const was = doc.dirty;
    doc.text = src.value; doc.dirty = doc.text !== doc.saved;
    if (doc.adds) doc.adds = null;
    queuePaint();
    if (was !== doc.dirty) { paintTabs(); paintTree(); }
  });
  src.addEventListener("keydown", (e) => {
    const mod = isMac ? e.metaKey : e.ctrlKey;
    if (mod && e.key.toLowerCase() === "s") { e.preventDefault(); saveAll(true); return; }
    if (e.key === "Tab" && !e.altKey && !mod) {
      e.preventDefault();
      const s = src.selectionStart, en = src.selectionEnd, v = src.value;
      if (e.shiftKey) {
        const ls = v.lastIndexOf("\n", s - 1) + 1;
        const cut = v.slice(ls, ls + 4).match(/^ {1,4}/);
        if (cut) { src.setRangeText("", ls, ls + cut[0].length, "preserve"); }
      } else { src.setRangeText("    ", s, en, "end"); }
      src.dispatchEvent(new Event("input")); return;
    }
    if (e.key === "Enter" && !mod && !e.shiftKey && !e.altKey) {
      // Keep the indent, and step in after an opening brace.
      const s = src.selectionStart, v = src.value;
      const ls = v.lastIndexOf("\n", s - 1) + 1;
      const indent = (v.slice(ls, s).match(/^\s*/) || [""])[0];
      const more = /[{(\[]\s*$/.test(v.slice(ls, s)) ? "    " : "";
      e.preventDefault();
      src.setRangeText("\n" + indent + more, s, src.selectionEnd, "end");
      src.dispatchEvent(new Event("input"));
    }
  });

  async function openFile(rel) {
    if (!ide) return;
    let doc = ide.docs.get(rel);
    if (!doc) {
      try {
        const text = await call("ide_read", { path: ide.path, rel });
        doc = { text, saved: text, dirty: false, adds: null, error: null };
      } catch (err) {
        doc = { text: "", saved: "", dirty: false, adds: null, error: String(err) };
      }
      ide.docs.set(rel, doc);
    }
    if (!ide.open.includes(rel)) ide.open.push(rel);
    ide.cur = rel;
    showDoc();
    paintTabs(); paintTree();
  }
  function showDoc() {
    const doc = ide && ide.cur ? ide.docs.get(ide.cur) : null;
    const code = $("ideCode"), msg = $("ideMsg"), empty = $("ideEmpty");
    empty.classList.toggle("hidden", !!doc);
    code.classList.toggle("hidden", !doc || !!doc.error);
    msg.classList.toggle("hidden", !doc || !doc.error);
    if (!doc) return;
    if (doc.error) { msg.textContent = `${ide.cur}: ${doc.error}`; return; }
    src.value = doc.text;
    src.scrollTop = 0; src.scrollLeft = 0;
    gut.innerHTML = "";
    paintCode();
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
      if (doc && doc.dirty) await saveOne(rel);
      ide.open = ide.open.filter((r) => r !== rel);
      if (ide.cur === rel) ide.cur = ide.open[ide.open.length - 1] || null;
      showDoc(); paintTabs(); paintTree(); return;
    }
    const t = e.target.closest("[data-rel]"); if (t && t.dataset.rel !== ide.cur) openFile(t.dataset.rel);
  });

  async function saveOne(rel) {
    const doc = ide.docs.get(rel); if (!doc || !doc.dirty || doc.error) return true;
    try {
      await call("ide_write", { path: ide.path, rel, text: doc.text });
      doc.saved = doc.text; doc.dirty = false; return true;
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

  /* ---- build, run, pack, ask ------------------------------------------ */
  function busy(on, label) {
    ide.busy = on;
    ["ideRun", "ideBuild", "ideAskGo", "ideRefresh"].forEach((id) => { const b = $(id); if (b) b.disabled = on; });
    $("ideAskBar").classList.toggle("busy", on);
    if (on) setStatus("busy", label || "Working…");
  }
  async function build(why) {
    if (!ide || ide.busy) return;
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
        problems([]);
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
  $("ideAskBar").addEventListener("submit", async (e) => {
    e.preventDefault();
    if (!ide || ide.busy) return;
    const request = askIn.value.trim();
    if (!request) { askIn.focus(); return; }
    await saveAll(false);
    const before = new Map([...ide.docs].map(([rel, d]) => [rel, d.text]));
    askIn.value = ""; askIn.placeholder = `${agentWords()} is changing the app…`;
    busy(true, `${agentWords()} is working…`);
    term(`==> asking ${agentWords()}: ${request}`);
    try {
      const r = await call("ide_ask", { path: ide.path, request, agent: agentName() });
      busy(false);
      const changed = (r && r.changed) || [];
      term(changed.length ? `==> changed ${changed.join(", ")}` : "==> nothing needed changing", "g");
      await loadTree();
      for (const rel of changed) {
        let text;
        try { text = await call("ide_read", { path: ide.path, rel }); } catch (err) { continue; }
        const old = before.get(rel);
        const doc = ide.docs.get(rel) || { text: "", saved: "", dirty: false };
        // The lines that are new, so the change can be seen where it landed.
        if (old != null) {
          const had = new Set(old.split("\n").map((l) => l.trim()));
          doc.adds = new Set(text.split("\n").map((l, i) => (l.trim() && !had.has(l.trim()) ? i : -1)).filter((i) => i >= 0));
        }
        doc.text = text; doc.saved = text; doc.dirty = false; doc.error = null;
        ide.docs.set(rel, doc);
        if (!ide.open.includes(rel)) ide.open.push(rel);
      }
      if (changed.length && !changed.includes(ide.cur)) ide.cur = changed.find((c) => /\.rs$/.test(c)) || changed[0];
      showDoc(); paintTabs(); paintTree();
      if (changed.length) build(`${changed[0]} changed`);
      else setStatus(ide.built ? "ok" : "idle", ide.built ? "Builds" : "Not built yet");
    } catch (err) {
      busy(false);
      setStatus("bad", "The change did not land");
      term(String(err), "r");
      problems([{ stage: "ask", message: String(err) }]);
    } finally {
      askIn.placeholder = askPh;
    }
  });
  $("ipTog").addEventListener("click", () => $("ideMain").classList.toggle("pmin"));
  view.querySelectorAll(".ip-tabs button[data-ip]").forEach((b) => b.addEventListener("click", () => panelTab(b.dataset.ip)));
  document.addEventListener("keydown", (e) => {
    if (!ide || view.classList.contains("hidden")) return;
    const mod = isMac ? e.metaKey : e.ctrlKey;
    if (mod && !e.shiftKey && e.key.toLowerCase() === "i") { e.preventDefault(); askIn.focus(); }
    if (mod && e.key.toLowerCase() === "s" && document.activeElement !== src) { e.preventDefault(); saveAll(true); }
  });
  $("ideAskKey").textContent = MOD + "I";

  /* ---- opening and leaving a project ------------------------------------ */
  async function openProject(p) {
    if (!p || !p.path) return;
    if (ide && ide.path !== p.path) await saveAll(false);
    ide = { path: p.path, name: p.name || p.path.split(/[\\/]/).pop(), tree: [], docs: new Map(), open: [], cur: null, busy: false, built: null, collapsed: new Set() };
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
  function hint(text) { const h = $("homeHint"); if (h) h.textContent = text; }

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
  window.addEventListener("beforeunload", () => { if (ide) saveAll(false); });
  // Studio's Code pane opens a built app's own source here.
  window.krIdeOpen = (path, name) => openProject({ path, name });
})();
