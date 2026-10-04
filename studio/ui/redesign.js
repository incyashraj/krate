/* Krate Studio, the 2026-10 design: the behaviours the new look adds.
 *
 * Loaded after app.js (and, on the web, after the bridge). Everything here
 * sits inside one function so it declares no top-level name: app.js and
 * bridge.js share one global scope in a browser tab, and a clash blanks the
 * page (scope-test.mjs).
 *
 * Two rules keep this from changing what Studio does:
 *  - it READS app.js's state (state, STAGES, tauri) and never writes it;
 *  - every action it offers presses one of app.js's own buttons, so the
 *    work is still done by the code that has always done it. Where the
 *    design folds two controls into one (Build/Plan into a menu, the voice
 *    button into the send button), the real controls stay in the page and
 *    are pressed on the person's behalf.
 * It never calls the backend itself.
 */
(function () {
  "use strict";
  const $ = (id) => document.getElementById(id);
  const q = (s, r = document) => r.querySelector(s);
  const qa = (s, r = document) => [...r.querySelectorAll(s)];
  const reduce = window.matchMedia && matchMedia("(prefers-reduced-motion: reduce)").matches;
  const ico = (id, cls = "kr-ico") => `<svg class="${cls}" aria-hidden="true" focusable="false"><use href="#i-${id}"/></svg>`;
  const app = () => { try { return state; } catch (e) { return null; } };
  const stages = () => { try { return STAGES; } catch (e) { return []; } };
  const desktopApp = () => { try { return !!tauri; } catch (e) { return false; } };
  const wide = () => window.matchMedia("(min-width: 861px)").matches;
  const mod = /mac/i.test(navigator.platform || navigator.userAgent) ? "⌘" : "Ctrl ";
  const visible = (el) => !!el && !el.classList.contains("hidden") && el.offsetParent !== null;
  const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));
  const watch = (el, opts, fn) => { if (el && window.MutationObserver) new MutationObserver(fn).observe(el, opts); };

  /* ---- a small menu that grows out of the control that opened it ------- */
  let popEl = null;
  function closePop() { if (popEl) { popEl.remove(); popEl = null; } }
  function openPop(anchor, items, opts = {}) {
    if (popEl && popEl._anchor === anchor) { closePop(); return; }
    closePop();
    const p = document.createElement("div");
    p.className = "pop kr-pop" + (opts.big ? " big" : "");
    p.setAttribute("role", "menu");
    p._anchor = anchor;
    for (const it of items) {
      if (it === "sep") { const s = document.createElement("div"); s.className = "kr-sep"; p.appendChild(s); continue; }
      if (it.head) {
        const h = document.createElement("div"); h.className = "kr-pop-head";
        h.innerHTML = it.head; p.appendChild(h); continue;
      }
      const b = document.createElement("button");
      b.type = "button"; b.className = "kr-pi" + (it.on ? " on" : "") + (it.danger ? " danger" : "");
      b.setAttribute("role", "menuitem");
      b.innerHTML = (it.mark ? `<span class="kr-lgb">${it.mark}</span>` : it.icon ? ico(it.icon) : "") +
        `<span class="kr-pt">${esc(it.label)}${it.sub ? `<small>${esc(it.sub)}</small>` : ""}</span>` +
        (it.on ? ico("check", "kr-ico kr-tick") : it.em ? `<em>${esc(it.em)}</em>` : "");
      b.addEventListener("click", (e) => { e.stopPropagation(); closePop(); it.run && it.run(); });
      p.appendChild(b);
    }
    document.body.appendChild(p);
    popEl = p;
    const r = anchor.getBoundingClientRect(), w = p.offsetWidth, h = p.offsetHeight;
    let x = Math.min(Math.max(8, r.left), innerWidth - w - 8);
    let y = r.bottom + 6;
    const above = opts.above || y + h > innerHeight - 8;
    if (above) y = r.top - h - 6;
    p.style.left = x + "px";
    p.style.top = Math.max(8, y) + "px";
    p.style.transformOrigin = `${r.left + r.width / 2 - x}px ${above ? "100%" : "0"}`;
  }
  document.addEventListener("pointerdown", (e) => {
    if (popEl && !popEl.contains(e.target) && e.target !== popEl._anchor && !(popEl._anchor && popEl._anchor.contains(e.target))) closePop();
  }, true);
  // Escape closes the menu and stops there: app.js also listens for it, and
  // would shut the sidebar behind the menu as well.
  document.addEventListener("keydown", (e) => { if (e.key === "Escape" && popEl) { closePop(); e.stopPropagation(); e.preventDefault(); } }, true);
  addEventListener("resize", closePop);

  /* Press one of app.js's own buttons without our interceptors catching it. */
  let bypass = false;
  function press(el) { if (!el) return; bypass = true; try { el.click(); } finally { bypass = false; } }

  /* ---- the boot mark ---------------------------------------------------- */
  (function boot() {
    if (reduce) return;
    const b = document.createElement("div");
    b.className = "kr-boot"; b.setAttribute("aria-hidden", "true");
    b.innerHTML = '<span class="kr-bm"><img src="krate-logo.png" alt=""></span>';
    document.body.appendChild(b);
    const t0 = performance.now();
    const gone = () => setTimeout(() => { b.classList.add("gone"); setTimeout(() => b.remove(), 700); }, Math.max(0, 650 - (performance.now() - t0)));
    if (document.readyState === "complete") gone(); else addEventListener("load", gone, { once: true });
  })();

  /* ---- Home: the word in the title, building then shipping -------------- */
  const rw = $("homeRw");
  if (rw) {
    const words = qa(".w", rw);
    let at = 0;
    const fit = () => { const w = words[at] ? words[at].offsetWidth : 0; if (w) rw.style.width = w + "px"; else rw.style.removeProperty("width"); };
    if (window.ResizeObserver) { const ro = new ResizeObserver(fit); words.forEach((w) => ro.observe(w)); }
    const show = (i) => {
      const cur = words[at];
      cur.classList.remove("on"); cur.classList.add("out");
      setTimeout(() => cur.classList.remove("out"), 700);
      at = i; words[i].classList.add("on"); fit();
    };
    (document.fonts && document.fonts.ready ? document.fonts.ready : Promise.resolve()).then(fit);
    addEventListener("resize", fit);
    if (!reduce && words.length > 1) setInterval(() => {
      const home = $("viewHome");
      if (document.hidden || !home || home.classList.contains("hidden")) return;
      show((at + 1) % words.length);
    }, 3200);
  }

  /* ---- Home: the placeholder types out ideas until someone types -------- */
  (function typingPlaceholder() {
    const ta = $("homePrompt");
    if (!ta || reduce) return;
    const wrap = document.createElement("div");
    wrap.className = "kr-ta";
    ta.parentNode.insertBefore(wrap, ta);
    wrap.appendChild(ta);
    const ph = document.createElement("div");
    ph.className = "kr-ph"; ph.setAttribute("aria-hidden", "true");
    wrap.appendChild(ph);
    wrap.classList.add("typing");
    const IDEAS = ["a habit tracker with streaks and a weekly chart", "split a dinner bill four ways, tip included",
      "a focus timer that lives in the corner", "flashcards for twenty Spanish words a day"];
    const lead = "Describe an app, like ";
    const sync = () => wrap.classList.toggle("has", ta.value.length > 0);
    ta.addEventListener("input", sync); sync();
    const wait = (ms) => new Promise((r) => setTimeout(r, ms));
    const paint = (s) => { ph.innerHTML = esc(s) + '<span class="kr-car"></span>'; };
    (async () => {
      for (let k = 0; ; k++) {
        const s = lead + IDEAS[k % IDEAS.length];
        for (let i = 0; i <= s.length; i++) {
          while (document.hidden) await wait(500);
          paint(s.slice(0, i)); await wait(i < lead.length ? 12 : 30);
        }
        await wait(2400);
        for (let i = s.length; i >= lead.length; i--) { paint(s.slice(0, i)); await wait(10); }
      }
    })();
  })();

  /* ---- Home: Create / Port --------------------------------------------- */
  const tabs = $("homeTabs");
  const pane = $("homePortPane");
  const bar = tabs ? tabs.closest(".bigbar") : null;
  const startPort = () => { const l = $("homePort"); if (l) l.click(); };
  function placeInd() {
    if (!tabs) return;
    const on = q("button.on", tabs), ind = q(".tab-ind", tabs);
    if (!on || !ind) return;
    ind.style.left = on.offsetLeft + 8 + "px";
    ind.style.width = Math.max(0, on.offsetWidth - 16) + "px";
  }
  function setMode(mode) {
    if (!tabs || !bar) return;
    qa("button[data-mode]", tabs).forEach((b) => {
      const on = b.dataset.mode === mode;
      b.classList.toggle("on", on);
      b.setAttribute("aria-selected", on ? "true" : "false");
    });
    const changed = bar.dataset.mode !== mode;
    bar.dataset.mode = mode;
    if (pane) pane.hidden = mode !== "port";
    const idePane = $("homeIdePane"), ideUnder = $("homeIdeUnder");
    if (idePane) idePane.hidden = mode !== "ide";
    if (ideUnder) ideUnder.hidden = mode !== "ide";
    const kb = $("homeKb");
    if (kb) {
      kb.hidden = mode === "port";
      kb.innerHTML = mode === "ide" ? `<kbd>${mod}I</kbd> asks your AI` : "<kbd>↵</kbd> to make";
    }
    placeInd();
    // The question changes with the door: building, porting, working on.
    const title = $("homeTitle");
    if (title && changed) {
      let alt = q(".kr-t-alt", title);
      if (!alt) {
        const main = document.createElement("span"); main.className = "kr-t-main";
        while (title.firstChild) main.appendChild(title.firstChild);
        title.appendChild(main);
        alt = document.createElement("span"); alt.className = "kr-t-alt"; title.appendChild(alt);
      }
      title.dataset.mode = mode;
      alt.textContent = mode === "port" ? "What are we porting?" : mode === "ide" ? "What are we working on?" : "";
      const swap = mode === "create" ? q(".kr-t-main", title) : alt;
      swap.classList.remove("kr-swap"); void swap.offsetWidth; swap.classList.add("kr-swap");
    }
    const sub = $("homeSub");
    if (sub && changed) {
      sub.textContent = mode === "port" ? "Krate reads it first and tells you what the port would take."
        : mode === "ide" ? "Edit a Krate project yourself, with your AI one keystroke away."
        : "Describe it. Krate turns it into one small file that opens on every desktop.";
      sub.classList.remove("kr-swap"); void sub.offsetWidth; sub.classList.add("kr-swap");
    }
    document.dispatchEvent(new CustomEvent("kr-home-mode", { detail: mode }));
    if (mode === "ide") { const f = $("homeIdeFind"); if (f) setTimeout(() => f.focus({ preventScroll: true }), 120); }
    // The sidebar lights the door that is open.
    const ideRow = $("sideIde"), homeRow = q('#side .side-row[data-side="home"]');
    if (ideRow && homeRow && !$("viewHome").classList.contains("hidden")) {
      ideRow.classList.toggle("on", mode === "ide");
      homeRow.classList.toggle("on", mode !== "ide");
    }
    if (mode === "create") { const ta = $("homePrompt"); if (ta) ta.focus({ preventScroll: true }); }
  }
  if (tabs) {
    tabs.addEventListener("click", (e) => { const b = e.target.closest("button[data-mode]"); if (b && !b.classList.contains("on")) setMode(b.dataset.mode); });
    (document.fonts && document.fonts.ready ? document.fonts.ready : Promise.resolve()).then(placeInd);
    addEventListener("resize", placeInd);
    setMode("create");
  }
  const go = $("homePortGo");
  if (go) go.addEventListener("click", startPort);

  /* ---- Home: the shelf's head is "Your apps (n)" and a chevron ---------- */
  (function shelfHead() {
    const grip = $("shelfGrip"), shelf = $("shelf");
    if (!grip || !shelf) return;
    const tog = document.createElement("button");
    tog.type = "button"; tog.className = "kr-shtog";
    tog.innerHTML = `<span>Your apps</span><em class="kr-shct"></em>${ico("chev", "kr-ico kr-shchev")}`;
    grip.insertAdjacentElement("afterend", tog);
    const sync = () => tog.setAttribute("aria-expanded", shelf.classList.contains("shut") ? "false" : "true");
    tog.addEventListener("click", () => {
      // Your apps, not Examples: the design's shelf is only what you made.
      const mine = $("tabMine"); if (mine && !mine.classList.contains("on")) press(mine);
      press(grip);
    });
    watch(shelf, { attributes: true, attributeFilter: ["class"] }, sync);
    sync();
  })();
  // The design's microphone on Home, where the old button drew a wave.
  (function mic() {
    const v = $("homeVoiceBtn");
    if (!v) return;
    v.classList.add("kr-micbtn");
    v.insertAdjacentHTML("afterbegin", ico("mic", "kr-ico kr-mic"));
  })();

  /* ---- the AI's own mark wherever the AI is named ---------------------- */
  function agentMark(name) {
    const n = String(name || "").toLowerCase();
    if (/claude|anthropic/.test(n)) return ico("claude", "kr-lgo");
    if (/codex|openai|gpt/.test(n)) return ico("openai", "kr-lgo");
    if (/gemini|google/.test(n)) return ico("gemini", "kr-lgo");
    if (/krate/.test(n)) return '<img class="kr-lgo" src="krate-logo.png" alt="">';
    // Still being looked up ("…"): no mark yet, rather than a ready-looking dot.
    if (!n.replace(/[.\u2026\s]/g, "")) return "";
    return '<span class="kr-lgo kr-lgo-dot"></span>';
  }
  function markChip(chip, nameEl) {
    if (!chip || !nameEl) return;
    let lg = q(".kr-lg", chip);
    if (!lg) { lg = document.createElement("span"); lg.className = "kr-lg"; chip.insertBefore(lg, chip.firstChild); }
    let chev = q(".kr-chev", chip);
    if (!chev) { chip.insertAdjacentHTML("beforeend", ico("chev", "kr-ico kr-chev")); }
    const paint = () => { const k = nameEl.textContent; if (lg.dataset.k !== k) { lg.dataset.k = k; lg.innerHTML = agentMark(k); } };
    paint();
    watch(nameEl, { childList: true, characterData: true, subtree: true }, paint);
  }
  markChip($("builtByChip"), $("builtByName"));
  markChip($("agentChip2"), $("agentName2"));
  markChip($("agentChip"), $("agentName"));

  /* ---- the AI chip opens a picker, not a sheet ------------------------- */
  // Each AI found on this computer, with its mark and whether it is ready.
  // A ready one is chosen right here (app.js's useAgent, the same call the
  // sheet's "Use this" makes); one that needs installing or a sign-in opens
  // the sheet, which is where that is done. API keys live in the sheet too.
  const logo = (name) => { try { return aiLogo(name); } catch (e) { return agentMark(name); } };
  function agentPicker(anchor) {
    const st = app(); if (!st) return;
    const API = new Set(["anthropic", "openai"]);
    const list = (st.agents || []).filter((a) => !API.has(a.name));
    const sheet = () => { try { openAiSheet(); } catch (e) {} };
    const items = [{ head: "Who writes your apps" }];
    const order = [...list.filter((a) => a.state === "working"), ...list.filter((a) => a.state !== "working")];
    for (const a of order) {
      const on = a.name === st.agent && a.state === "working";
      const sub = a.state === "working" ? (on ? "Ready · writing your apps" : "Ready")
        : a.state === "missing" ? "Not installed"
        : a.state === "paused" ? "Paused just now"
        : /sign/i.test(a.detail || "") ? "Needs a sign-in" : "Needs a one-time fix";
      items.push({
        mark: logo(a.name), label: a.label || a.name, sub, on,
        em: a.state === "missing" ? "Install" : a.state === "working" ? "" : "Fix",
        run: async () => {
          if (a.state !== "working") return sheet();
          if (on) return;
          try { await useAgent(a.name); toast(`${a.label || a.name} will write your apps`); } catch (e) { sheet(); }
        },
      });
    }
    if (!order.length) {
      items.push(st.agentsError
        ? { icon: "info", label: "Krate could not look for AIs", sub: "See why", run: sheet }
        : { icon: "refresh", label: "Looking for your AIs", sub: "A moment", run: () => { try { refreshAgents(); } catch (e) {} } });
    }
    if (desktopApp()) items.push("sep", { icon: "key", label: "Use an API key", sub: "Anthropic, OpenAI or Google", run: sheet });
    else items.push("sep", { icon: "gear", label: "More about your AI", run: sheet });
    openPop(anchor, items, { big: true, above: anchor.getBoundingClientRect().top > innerHeight / 2 });
  }
  ["agentChip", "agentChip2", "builtByChip"].forEach((id) => {
    const chip = $(id);
    if (!chip) return;
    chip.setAttribute("aria-haspopup", "menu");
    chip.addEventListener("click", (e) => {
      if (bypass) return;
      e.stopImmediatePropagation(); e.preventDefault();
      agentPicker(chip);
    }, true);
  });

  /* ---- Build / Plan as one chip with a menu ---------------------------- */
  function modeChip(seg, opts = {}) {
    if (!seg) return;
    const btns = qa("button", seg);
    const build = btns.find((b) => /build/i.test(b.textContent));
    const plan = btns.find((b) => /plan/i.test(b.textContent));
    if (!build || !plan) return;
    const chip = document.createElement("button");
    chip.type = "button";
    chip.className = "kr-mode" + (opts.compact ? " compact" : "");
    seg.parentNode.insertBefore(chip, seg);
    seg.classList.add("kr-folded");
    const isPlan = () => plan.getAttribute("aria-pressed") === "true" || plan.classList.contains("on");
    const paint = () => {
      const p = isPlan();
      chip.classList.toggle("plan", p);
      chip.innerHTML = `<span class="mv">${p ? "Plan first" : "Build"}</span>` + ico("chev", "kr-ico kr-chev");
      chip.title = p ? "Krate shows a plan before it makes anything" : "Krate makes it straight away";
    };
    paint();
    btns.forEach((b) => watch(b, { attributes: true, attributeFilter: ["aria-pressed", "class"] }, paint));
    chip.addEventListener("click", () => openPop(chip, [
      { label: "Build", sub: "Make it straight away", on: !isPlan(), run: () => press(build) },
      { label: "Plan first", sub: "See the plan before anything is made", on: isPlan(), run: () => press(plan) },
    ], { big: true }));
    return { isPlan, build, plan };
  }
  const homeBar = $("homePrompt") ? $("homePrompt").closest(".bigbar") : null;
  const sessBar = $("prompt") ? $("prompt").closest(".bigbar") : null;
  const homeModes = modeChip(homeBar && q(".web-mode", homeBar));
  const sessModes = modeChip(sessBar && q(".web-mode", sessBar), { compact: true });

  /* ---- + opens a menu: add a file, or plan before building -------------- */
  function plusMenu(btn, modes, extra) {
    if (!btn) return;
    btn.addEventListener("click", (e) => {
      if (bypass) return;
      e.stopImmediatePropagation(); e.preventDefault();
      const items = [{ icon: "file", label: "Add a file or a picture", sub: "The AI reads it with your words", run: () => press(btn) }];
      if (extra) items.push(...extra);
      if (modes) items.push("sep", { icon: "spark", label: "Plan before building", on: modes.isPlan(), run: () => press(modes.isPlan() ? modes.build : modes.plan) });
      openPop(btn, items);
    }, true);
  }
  plusMenu($("homeAttachBtn"), homeModes, [{ icon: "port", label: "Port an app you already have", run: () => setMode("port") }]);
  plusMenu($("attachBtn"), sessModes);

  /* ---- the send button: a wave when there is nothing to send ------------ */
  const canSpeak = !!(window.SpeechRecognition || window.webkitSpeechRecognition);
  function sendButton(send, field, voice, opts = {}) {
    if (!send || !field) return;
    send.insertAdjacentHTML("beforeend", ico("wave", "kr-ico kr-wave"));
    if (!send.querySelector(".send-arrow")) {
      const svg = send.querySelector("svg:not(.kr-wave)");
      if (svg) svg.classList.add("send-arrow");
    }
    const paint = () => {
      const empty = field.value.length === 0;
      const idle = empty && !send.classList.contains("stopping");
      send.classList.toggle("kr-voice", !!(opts.voice && canSpeak && voice && idle));
      send.classList.toggle("kr-listen", !!(voice && voice.classList.contains("listening")));
    };
    field.addEventListener("input", paint);
    watch(send, { attributes: true, attributeFilter: ["class"] }, () => { if (!send._p) { send._p = 1; paint(); send._p = 0; } });
    if (voice) watch(voice, { attributes: true, attributeFilter: ["class"] }, paint);
    paint();
    if (opts.voice) send.addEventListener("click", (e) => {
      if (bypass || !send.classList.contains("kr-voice") && !send.classList.contains("kr-listen")) return;
      e.stopImmediatePropagation(); e.preventDefault();
      press(voice);
    }, true);
  }
  sendButton($("send"), $("prompt"), $("voiceBtn"), { voice: true });
  sendButton($("homeSend"), $("homePrompt"), $("homeVoiceBtn"));

  /* ---- the sidebar ------------------------------------------------------ */
  const side = $("side");
  const nav = side && q(".side-nav", side);
  // Search sits second in the list, under New app, the way the design has it.
  (function moveSearch() {
    const search = side && q(".side-search", side);
    const first = nav && q(".side-row", nav);
    if (!search || !first) return;
    first.insertAdjacentElement("afterend", search);
    const kbd = q(".side-key", search);
    if (kbd) kbd.textContent = mod + "K";
    // In the design, Search is a row that opens the search window, not a
    // field. The field stays (app.js filters Recents with it and "/" focuses
    // it) but taking focus now opens search instead.
    const field = $("sideSearch");
    if (field) {
      field.readOnly = true;
      field.setAttribute("aria-haspopup", "dialog");
      field.addEventListener("focus", () => { field.blur(); paletteOpen(); });
    }
    search.addEventListener("click", () => paletteOpen());
    const gap = document.createElement("span"); gap.className = "kr-gap"; search.insertAdjacentElement("afterend", gap);
  })();
  // One indicator that slides to whichever row is current.
  const ind = document.createElement("span");
  ind.className = "kr-ind"; ind.setAttribute("aria-hidden", "true");
  if (nav) nav.insertBefore(ind, nav.firstChild);
  function placeNav() {
    if (!nav) return;
    const on = q(":scope > .side-row.on", nav);
    if (!on || !on.offsetHeight) { ind.style.opacity = 0; return; }
    ind.style.opacity = 1;
    ind.style.height = on.offsetHeight + "px";
    ind.style.transform = `translateY(${on.offsetTop}px)`;
  }
  watch(nav, { attributes: true, subtree: true, attributeFilter: ["class"] }, placeNav);
  addEventListener("resize", placeNav);
  (document.fonts && document.fonts.ready ? document.fonts.ready : Promise.resolve()).then(placeNav);
  // Names for the rows, for the icon rail and for screen readers alike.
  qa(".side-row", side || document).forEach((r) => { const t = q("span", r); if (t && !r.dataset.tip) r.dataset.tip = t.textContent.trim(); });
  const toggleBtn = $("sideCloseToggle");
  if (toggleBtn) toggleBtn.dataset.tip = "Show the sidebar";
  // The design's head: the mark on the left, the panel toggle on the right,
  // one row under the window's traffic lights.
  const brandRow = side && q(".side-brandrow", side);
  if (toggleBtn && brandRow) brandRow.appendChild(toggleBtn);

  // How many apps you have made, beside Your apps: files, deduped by path,
  // the same count the Your apps page shows.
  (function appCount() {
    const row = q('#side .side-row[data-side="all"]');
    if (!row) return;
    const ct = document.createElement("span"); ct.className = "kr-ct"; row.appendChild(ct);
    let t = 0;
    const count = async () => {
      let list = [];
      try { list = (await invoke("sessions_list")) || []; } catch (e) { return; }
      const n = new Set(list.filter((x) => x.result && x.result.path).map((x) => x.result.path)).size;
      ct.textContent = n ? String(n) : "";
      const shelfCt = q("#shelf .kr-shct"); if (shelfCt) shelfCt.textContent = n ? String(n) : "";
    };
    const soon = () => { clearTimeout(t); t = setTimeout(count, 400); };
    watch($("shelfBody"), { childList: true }, soon);
    watch($("sideSessions"), { childList: true }, soon);
    soon();
  })();

  // On a wide window a shut sidebar becomes a rail of icons, every icon in
  // its place; on a narrow one it still slides away entirely.
  function railSync() {
    if (!side) return;
    const rail = wide() && side.dataset.open !== "true";
    document.body.classList.toggle("kr-rail", rail);
    if (rail && side.inert) side.inert = false;
    setTimeout(placeNav, 520);
  }
  watch(side, { attributes: true, attributeFilter: ["data-open"] }, railSync);
  addEventListener("resize", railSync);
  railSync();
  // A name beside an icon on the rail, after a short hover.
  const tip = document.createElement("div");
  tip.className = "kr-tip"; tip.setAttribute("aria-hidden", "true");
  document.body.appendChild(tip);
  let tipT = 0;
  const hideTip = () => { clearTimeout(tipT); tip.classList.remove("on"); };
  if (side) {
    side.addEventListener("mouseover", (e) => {
      const t = e.target.closest("[data-tip]");
      if (!t || !document.body.classList.contains("kr-rail")) { hideTip(); return; }
      clearTimeout(tipT);
      tipT = setTimeout(() => {
        const r = t.getBoundingClientRect();
        tip.textContent = t.dataset.tip;
        tip.style.top = r.top + r.height / 2 + "px";
        tip.style.left = r.right + 10 + "px";
        tip.classList.add("on");
      }, tip.classList.contains("on") ? 0 : 280);
    });
    side.addEventListener("mouseleave", hideTip);
    side.addEventListener("click", hideTip);
  }

  // Recents: the one being made now pulses; a new one slides in.
  const sessList = $("sideSessions");
  let seen = null;
  function paintRecents() {
    if (!sessList) return;
    const rows = qa(".sess-row", sessList);
    const building = app() && app().buildingSession ? app().buildingSession.id : null;
    const ids = new Set();
    for (const r of rows) {
      ids.add(r.dataset.id);
      r.classList.toggle("kr-live", !!building && r.dataset.id === building);
      if (seen && !seen.has(r.dataset.id) && !r.dataset.krIn) { r.dataset.krIn = "1"; r.classList.add("kr-enter"); }
      if (!r.dataset.tip) { const n = q(".sess-name", r); if (n) r.dataset.tip = n.textContent; }
    }
    if (rows.length) seen = ids;
  }
  watch(sessList, { childList: true }, paintRecents);
  setInterval(paintRecents, 1000);

  // The foot: you, and a menu with everything that used to be rows.
  (function me() {
    const acct = $("sideAccount");
    if (!acct) return;
    const mt = document.createElement("span");
    mt.className = "kr-mt";
    mt.innerHTML = '<b class="kr-mn"></b><small class="kr-mp"></small>';
    acct.appendChild(mt);
    const paint = () => {
      const a = (app() && app().account) || null;
      let name = (a && (a.name || a.login)) || "";
      try { if (!name) name = localStorage.getItem("krate-name") || ""; } catch (e) {}
      q(".kr-mn", mt).textContent = name || "You";
      q(".kr-mp", mt).textContent = a && a.login ? "@" + a.login : desktopApp() ? "On this computer" : "Not signed in";
      acct.dataset.tip = name || "You";
    };
    paint(); setInterval(paint, 2000);
    acct.addEventListener("click", (e) => {
      if (bypass) return;
      e.stopImmediatePropagation(); e.preventDefault();
      const dark = !document.body.classList.contains("light");
      const items = [
        { head: `<span class="kr-av">${esc((q(".kr-mn", mt).textContent || "Y").charAt(0).toUpperCase())}</span><span><b>${esc(q(".kr-mn", mt).textContent)}</b><small>${esc(q(".kr-mp", mt).textContent)}</small></span>` },
        "sep",
        { icon: "user", label: "Profile", run: () => press(acct) },
        { icon: "gear", label: "Settings", run: () => press($("sideSettings")) },
        { icon: dark ? "sun" : "moon", label: dark ? "Light mode" : "Dark mode", run: () => press($("themeBtn")) },
        { icon: "gift", label: "Share Krate", run: () => press($("sideShare")) },
        { icon: "book", label: "Docs", em: "↗", run: () => press($("sideDocs")) },
      ];
      openPop(acct, items, { above: true });
    }, true);
  })();

  /* ---- search everything: ⌘K ------------------------------------------- *
   * The design's search: Studio's actions with their keys, your apps with
   * their pictures and sizes, and -- once the Gallery has been loaded -- the
   * apps in it. Every row does what the matching control in Studio does. */
  let paletteOpen = () => {};
  (function palette() {
    const scrim = document.createElement("div"); scrim.className = "kr-scrim";
    const pal = document.createElement("div"); pal.className = "kr-pal"; pal.setAttribute("role", "dialog"); pal.setAttribute("aria-label", "Search");
    pal.innerHTML = `<div class="kr-pin">${ico("search")}<input type="text" placeholder="Search apps, actions and the gallery" aria-label="Search" autocomplete="off" spellcheck="false"><kbd>esc</kbd></div><div class="kr-pres"></div><div class="kr-pf"><span>↑↓ to move</span><span>↵ to open</span><span>${mod}K anywhere</span></div>`;
    document.body.append(scrim, pal);
    const input = q("input", pal), res = q(".kr-pres", pal);
    let items = [], hl = 0, mine = [];
    const isOn = () => pal.classList.contains("on");
    const rowClick = (sel) => () => { const r = q(sel); if (r) r.click(); };
    const appName = (s) => ((s.result && s.result.name) || s.title || "App").replace(/\.krate$/, "").replace(/[-_]+/g, " ").replace(/\b\w/g, (c) => c.toUpperCase());
    const shotOf = new Map();
    async function loadMine() {
      let list = [];
      try { list = (await invoke("sessions_list")) || []; } catch (e) { list = []; }
      const seen = new Set();
      mine = list.filter((x) => x.result && x.result.path && !seen.has(x.result.path) && seen.add(x.result.path))
        .sort((a, b) => (b.updated || 0) - (a.updated || 0));
      for (const x of mine.slice(0, 12)) {
        if (shotOf.has(x.id)) continue;
        const sh = x.result.shot;
        if (sh && sh !== "file") shotOf.set(x.id, sh);
        else if (sh === "file") invoke("session_shot", { id: x.id }).then((d) => { if (d) { shotOf.set(x.id, d); if (isOn()) render(); } }).catch(() => {});
      }
      if (isOn()) render();
    }
    function render() {
      const s = input.value.trim().toLowerCase();
      const dark = !document.body.classList.contains("light");
      const acts = [
        ["New app", "new", rowClick('#side .side-row[data-side="home"]'), mod + "N"],
        ["Port an app", "port", rowClick("#sidePort"), ""],
        ["Open the IDE", "code", rowClick("#sideIde"), ""],
        ["Settings", "gear", () => press($("sideSettings")), mod + ","],
        [dark ? "Switch to light" : "Switch to dark", dark ? "sun" : "moon", () => press($("themeBtn")), ""],
        ["Gallery", "compass", rowClick('#side .side-row[data-side="discover"]'), ""],
      ].filter((a) => (a[0] !== "Open the IDE" || $("sideIde")) && a[0].toLowerCase().includes(s))
        .map((a) => ({ g: "Actions", t: a[0], i: ico(a[1]), k: a[3], f: a[2] }));
      const apps = mine.filter((x) => appName(x).toLowerCase().includes(s) || (x.title || "").toLowerCase().includes(s)).slice(0, 8)
        .map((x) => ({ g: "Your apps", t: appName(x), i: `<span class="kr-th0">${shotOf.has(x.id) ? `<img src="${esc(shotOf.get(x.id))}" alt="">` : ""}</span>`, k: x.result.size || "", f: () => { try { openSession(x); } catch (e) {} } }));
      const st = app();
      const gal = s && st && Array.isArray(st.cloud) ? st.cloud.filter((a) => ((a.meta && a.meta.name) || "").toLowerCase().includes(s)).slice(0, 5)
        .map((a) => ({ g: "Gallery", t: a.meta.name, i: `<span class="kr-th0">${a.shot ? `<img src="${esc(a.shot)}" alt="">` : ""}</span>`, k: a.meta.author ? "@" + a.meta.author : "", f: () => { try { showCloudApp(a); } catch (e) {} } })) : [];
      items = [...acts, ...apps, ...gal];
      if (s && !items.length) items = [{ g: "Make it", t: `Make “${input.value.trim()}”`, i: ico("spark"), k: "↵", f: () => {
        rowClick('#side .side-row[data-side="home"]')();
        setTimeout(() => { const ta = $("homePrompt"); if (ta) { ta.value = input.value.trim(); ta.dispatchEvent(new Event("input")); ta.focus(); } }, 200);
      } }];
      hl = Math.max(0, Math.min(hl, items.length - 1));
      let g = "";
      res.innerHTML = items.map((it, i) => (it.g !== g ? `<div class="kr-ph2">${esc((g = it.g))}</div>` : "") +
        `<button type="button" class="kr-pi${i === hl ? " hl" : ""}" data-i="${i}">${it.i}<span class="kr-pt">${esc(it.t)}</span>${it.k ? `<em>${esc(it.k)}</em>` : ""}</button>`).join("");
    }
    function open() { closePop(); input.value = ""; hl = 0; render(); loadMine(); scrim.classList.add("on"); pal.classList.add("on"); setTimeout(() => input.focus(), 30); }
    function close() { scrim.classList.remove("on"); pal.classList.remove("on"); }
    paletteOpen = open;
    function pick(i) { const it = items[i]; close(); if (it) it.f(); }
    input.addEventListener("input", () => { hl = 0; render(); });
    input.addEventListener("keydown", (e) => {
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault(); if (!items.length) return;
        hl = (hl + (e.key === "ArrowDown" ? 1 : -1) + items.length) % items.length; render();
        const h = q(".hl", res); if (h) h.scrollIntoView({ block: "nearest" });
      }
      if (e.key === "Enter") { e.preventDefault(); pick(hl); }
      if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); close(); }
    });
    res.addEventListener("click", (e) => { const b = e.target.closest("[data-i]"); if (b) pick(+b.dataset.i); });
    res.addEventListener("mousemove", (e) => { const b = e.target.closest("[data-i]"); if (b && +b.dataset.i !== hl) { hl = +b.dataset.i; qa(".kr-pi", res).forEach((x) => x.classList.toggle("hl", x === b)); } });
    scrim.addEventListener("click", close);
    document.addEventListener("keydown", (e) => {
      const m = (e.metaKey || e.ctrlKey) && !e.altKey;
      if (!m) return;
      const k = e.key.toLowerCase();
      if (k === "k") { e.preventDefault(); isOn() ? close() : open(); }
      // The keys the design shows beside these actions.
      if (k === "n" && !e.shiftKey) { e.preventDefault(); close(); rowClick('#side .side-row[data-side="home"]')(); setTimeout(() => { const t = $("homePrompt"); if (t) t.focus(); }, 250); }
      if (k === ",") { e.preventDefault(); close(); press($("sideSettings")); }
    });
  })();

  /* ---- the session's header: the version, and tabs with a sliding thumb */
  (function header() {
    const title = $("sessTitleBtn");
    if (title) {
      const ver = document.createElement("span");
      ver.className = "kr-ver"; ver.hidden = true;
      title.insertAdjacentElement("afterend", ver);
      const paint = () => {
        const chips = qa("#thread .vchip b");
        const last = chips[chips.length - 1];
        ver.hidden = !last;
        if (last && ver.textContent !== last.textContent) ver.textContent = last.textContent;
      };
      watch($("thread"), { childList: true, subtree: true }, paint);
      paint();
    }
    const pt = $("panelTabs");
    if (pt) {
      const th = document.createElement("span");
      th.className = "kr-thumb"; th.setAttribute("aria-hidden", "true");
      pt.insertBefore(th, pt.firstChild);
      const place = () => {
        const on = q("button.on", pt);
        if (!on || !on.offsetWidth) return;
        th.style.left = on.offsetLeft + "px"; th.style.width = on.offsetWidth + "px";
        // add() always records a change, even when the class is there, and
        // this element sits inside what is being watched: guard it or it loops.
        if (!th.classList.contains("ready")) th.classList.add("ready");
      };
      watch(pt, { attributes: true, subtree: true, attributeFilter: ["class"] }, place);
      watch($("viewSession"), { attributes: true, attributeFilter: ["class"] }, () => requestAnimationFrame(place));
      addEventListener("resize", place);
      (document.fonts && document.fonts.ready ? document.fonts.ready : Promise.resolve()).then(place);
    }
    const share = $("barShare");
    // Run it is relabelled by app.js (textContent), so its icon is drawn in CSS.
    if (share && !q("svg", share)) share.insertAdjacentHTML("afterbegin", ico("share"));
  })();

  /* ---- the conversation ------------------------------------------------- */
  const thread = $("thread");
  // Krate's words arrive a word at a time, the way they are being said.
  // Only a message that arrives on its own: a reopened session appends its
  // whole history in one go, and replaying that would be a slow show.
  function wordIn(msg) {
    const body = q(":scope > .body", msg);
    if (!body || reduce || body.dataset.krw) return;
    const text = body.textContent;
    if (!text || text.length > 900) return;
    body.dataset.krw = "1";
    body.textContent = "";
    text.split(/(\s+)/).forEach((part, i) => {
      if (!part) return;
      if (/^\s+$/.test(part)) { body.appendChild(document.createTextNode(part)); return; }
      const s = document.createElement("span");
      s.className = "kr-w"; s.style.animationDelay = Math.min(i, 160) * 12 + "ms";
      s.textContent = part; body.appendChild(s);
    });
  }
  // A plan and a question are cards, not lines. The words are app.js's own
  // (the planner writes at most three sentences; questions come numbered,
  // "1. ", "2. "); the card only sets them out, and its buttons press the
  // ones app.js put under the message. The message's own text stays in the
  // page, hidden, so copying and replaying read what they always read.
  const PLAN_HEAD = /^Here's what I'll build:\s*/;
  function appName() {
    // A name the person gave (a rename, or the built app's) is used as it
    // is. Otherwise the request is the title, and a request reads as a
    // sentence, so the card names the thing: "a tip splitter for dinners"
    // is a Tip Splitter.
    const t = (($("railTitle") || {}).textContent || "").trim();
    const first = q(".msg.you .body", thread);
    const asked = first ? first.textContent.trim() : "";
    if (t && t !== "New app" && t !== asked) return t;
    const p = asked.replace(/^(please\s+)?(make|build|create|write)\s+(me\s+)?/i, "").replace(/^(a|an|the|my)\s+/i, "");
    const w = p.split(/\s+(?:with|that|for|which|to|where|using|so|in|on|and)\s+/i)[0].split(/\s+/).slice(0, 4).join(" ").replace(/[.,;:!?]+$/, "");
    return w ? w.replace(/\b\w/g, (c) => c.toUpperCase()) : "Your app";
  }
  function sentences(s) {
    return s.replace(/\s+/g, " ").trim().split(/(?<=[.!?])\s+(?=[A-Z0-9"'(])/).map((x) => x.replace(/[.]$/, "").trim()).filter(Boolean);
  }
  function cardButtons(msg, card, onPick) {
    const row = q(":scope > .msg-actions", msg);
    const acts = document.createElement("div"); acts.className = "kr-acts";
    if (row) {
      qa("button", row).forEach((b) => {
        const nb = document.createElement("button");
        nb.type = "button";
        nb.className = "btn " + (b.classList.contains("btn-primary") ? "kr-dark" : "kr-ghost");
        nb.textContent = b.textContent;
        nb.addEventListener("click", () => { onPick && onPick(b, acts); b.click(); });
        nb._real = b;
        acts.appendChild(nb);
      });
      row.hidden = true;
    }
    card.appendChild(acts);
    return acts;
  }
  function settle(acts, words) {
    acts.innerHTML = `<span class="kr-tr done kr-went"><span class="kr-tic"><span class="kr-ck">${ico("check", "")}</span></span>${esc(words)}</span>`;
  }
  function planCard(msg, body) {
    const port = msg.classList.contains("plan") && !PLAN_HEAD.test(body);
    let text = body.replace(PLAN_HEAD, "");
    let needs = [];
    const nm = text.match(/\n\n(?:The ported app|It) will ask your permission to: ([^\n]*)\.?\s*$/m);
    if (nm) { needs = nm[1].replace(/\.$/, "").split(/;\s*/).filter(Boolean); text = text.replace(nm[0], ""); }
    let items, lead = "";
    if (port) {
      const parts = text.split(/\n\n/);
      lead = parts[0] || "";
      items = (text.match(/^•\s*(.+)$/gm) || []).map((l) => l.replace(/^•\s*/, ""));
      if (!items.length && /Nothing in it needs changing/.test(text)) items = ["Nothing in it needs changing: it ports as it is"];
      items.push("Your original is not touched");
    } else {
      items = sentences(text);
    }
    const card = document.createElement("div");
    card.className = "kr-plan";
    card.innerHTML = `<span class="kr-ptag">${ico("spark", "kr-ico")}${port ? "Port plan" : "Plan"}</span><h4>${esc(appName())}</h4>` +
      (lead ? `<p class="kr-plead">${esc(lead)}</p>` : "") + "<ul></ul>" +
      (needs.length ? `<div class="kr-pneeds"><span>It will ask first to</span>${needs.map((n) => `<em>${esc(n)}</em>`).join("")}</div>` : "");
    const ul = q("ul", card);
    items.forEach((t, i) => {
      const li = document.createElement("li"); li.textContent = t;
      li.style.animationDelay = (reduce ? 0 : 120 + i * 150) + "ms";
      ul.appendChild(li);
    });
    const acts = cardButtons(msg, card, (b, a) => settle(a, port ? "Porting this plan" : "Building this plan"));
    if (acts.children.length && !port) {
      // The second way out: change the plan in words. The next message is
      // the final word on it (app.js, runPlan), so the box says so.
      const ch = document.createElement("button");
      ch.type = "button"; ch.className = "btn kr-ghost"; ch.textContent = "Change something";
      ch.addEventListener("click", () => { const p = $("prompt"); if (p) { p.placeholder = "What should change in the plan?"; p.focus(); } });
      acts.appendChild(ch);
    }
    if (acts.children.length) {
      const b0 = acts.children[0]; if (/^Build it$/.test(b0.textContent)) b0.textContent = "Build this";
    } else acts.remove();
    msg.classList.add("kr-carded", "kr-plan-msg");
    msg.appendChild(card);
  }
  function questionCard(msg, body) {
    const qs = body.split("\n").map((l) => l.replace(/^\s*\d+\.\s*/, "").trim()).filter(Boolean);
    const card = document.createElement("div");
    card.className = "kr-qc";
    card.innerHTML = `<span class="kr-qtag">${ico("msg", "kr-ico")}${qs.length > 1 ? `${qs.length} questions before I build` : "One question before I build"}</span>` +
      qs.map((t, i) => `<b class="kr-qq" style="animation-delay:${reduce ? 0 : 80 + i * 120}ms">${qs.length > 1 ? `<i>${i + 1}</i>` : ""}<span>${esc(t)}</span></b>`).join("");
    const acts = cardButtons(msg, card, (b, a) => settle(a, "Building without an answer"));
    const ans = document.createElement("button");
    ans.type = "button"; ans.className = "btn kr-dark"; ans.textContent = qs.length > 1 ? "Answer them" : "Answer it";
    ans.addEventListener("click", () => { const p = $("prompt"); if (p) { p.disabled = false; p.focus(); } });
    acts.prepend(ans);
    qa("button", acts).forEach((b) => { if (b._real && /^Build it$/.test(b.textContent)) { b.textContent = "Skip and build"; b.title = "Krate picks for you; you can change it after"; } });
    msg.classList.add("kr-carded", "kr-q-msg");
    msg.appendChild(card);
  }
  function cardify(n) {
    if (!n.matches || !n.matches(".msg.krate") || n.classList.contains("kr-carded")) return false;
    const body = (q(":scope > .body", n) || {}).textContent || "";
    if (n.classList.contains("plan") || PLAN_HEAD.test(body)) { planCard(n, body); return true; }
    if (/^\s*1\.\s/.test(body)) { questionCard(n, body); return true; }
    return false;
  }
  // A typed answer or a build makes the old card's buttons stale: app.js
  // removes its own row then (clearAnsweredActions), and the card follows.
  function syncCards() {
    qa(".kr-carded", thread).forEach((m) => {
      const a = q(".kr-acts", m);
      if (!a || q(".kr-went", a)) return;
      const live = qa("button", a).some((b) => !b._real || b._real.isConnected);
      const anyReal = qa("button", a).some((b) => b._real);
      if (anyReal && !qa("button", a).some((b) => b._real && b._real.isConnected)) {
        if (m.classList.contains("kr-plan-msg") && q(".msg.vlive, .msg.vok", thread)) settle(a, "Building this plan");
        else a.remove();
      } else if (!live) a.remove();
    });
  }
  if (thread) qa(".msg.krate", thread).forEach(cardify);
  watch(thread, { childList: true }, (recs) => {
    const added = [];
    recs.forEach((r) => r.addedNodes.forEach((n) => { if (n.nodeType === 1) added.push(n); }));
    added.forEach(cardify);
    syncCards();
    if (added.length > 2) return;
    added.forEach((n) => {
      n.classList.add("kr-in");
      if (n.matches(".msg.krate") && !n.matches(".vlive, .vok, .vbad, .kr-carded")) wordIn(n);
    });
  });

  // The build's steps as rows under its chip: a spinner on the one being
  // done, a tick on each one finished. Read from the build's own stage.
  const STEP_WORDS = { read: "Reading Krate's API", write: "Writing the code", test: "Building and testing it", done: "Packing the file" };
  let toolsFor = null, toolsHtml = "";
  function toolRow(label, st) {
    const tic = st === "done" ? `<span class="kr-ck">${ico("check", "")}</span>` : st === "bad" ? '<span class="kr-bad">!</span>' : st === "stop" ? '<span class="kr-stp"></span>' : '<span class="kr-spin"></span>';
    return `<div class="kr-tr${st === "done" ? " done" : ""}" data-st="${st}"><span class="kr-tic">${tic}</span><span>${esc(label)}</span></div>`;
  }
  function paintTools() {
    if (!thread) return;
    const live = q(".msg.vlive", thread);
    const st = app();
    if (live) {
      toolsFor = live;
      const idx = st ? st.stageIndex : -1;
      const list = stages();
      let box = q(":scope > .kr-tools", live);
      if (!box) {
        box = document.createElement("div"); box.className = "kr-tools";
        const chip = q(".vchip", live); (chip || live).insertAdjacentElement("afterend", box);
      }
      const rows = list.slice(0, Math.max(1, idx + 1)).map((s, i) => toolRow(STEP_WORDS[s.key] || s.label, i < idx ? "done" : "now"));
      const html = rows.join("");
      if (box.dataset.h !== html) {
        // Only the rows that changed are replaced, so a finished row does
        // not replay its tick every second.
        const have = qa(".kr-tr", box);
        rows.forEach((h, i) => {
          if (have[i] && have[i].outerHTML === h) return;
          const t = document.createElement("template"); t.innerHTML = h;
          if (have[i]) have[i].replaceWith(t.content.firstChild); else box.appendChild(t.content.firstChild);
        });
        box.dataset.h = html;
        toolsHtml = html;
      }
      return;
    }
    // Settled: the chip was rewritten. Put the rows back, finished.
    if (toolsFor && toolsFor.isConnected && !q(":scope > .kr-tools", toolsFor) && toolsHtml) {
      const ok = toolsFor.classList.contains("vok");
      const stopped = /stopped/.test(toolsFor.textContent);
      const box = document.createElement("div"); box.className = "kr-tools settled";
      const t = document.createElement("template"); t.innerHTML = toolsHtml;
      const rows = [...t.content.children];
      rows.forEach((r, i) => {
        const last = i === rows.length - 1;
        const s = ok || !last ? "done" : stopped ? "stop" : "bad";
        const label = r.textContent;
        const n = document.createElement("template"); n.innerHTML = toolRow(label, s);
        box.appendChild(n.content.firstChild);
      });
      const chip = q(".vchip", toolsFor); (chip || toolsFor).insertAdjacentElement("afterend", box);
      toolsFor = null; toolsHtml = "";
    }
  }
  setInterval(() => { paintTools(); syncCards(); }, 400);
  watch(thread, { childList: true, subtree: false }, paintTools);

  /* ---- the card being made: the crate ----------------------------------- */
  let crateN = 0;
  function crateSvg() {
    const k = "c" + (++crateN);
    const shell = (y) => {
      const faces = (fills) =>
        `<path class="l" ${fills[0]}d="M28 ${y + 40} L80 ${y + 66} L80 ${y + 81} L28 ${y + 55}Z"/>` +
        `<path class="r" ${fills[1]}d="M80 ${y + 66} L132 ${y + 40} L132 ${y + 55} L80 ${y + 81}Z"/>` +
        `<path class="t" ${fills[2]}d="M28 ${y + 40} L80 ${y + 14} L132 ${y + 40} L80 ${y + 66}Z"/>`;
      return `<g class="sk">${faces(["", "", ""])}</g>` +
        `<g class="fl">${faces([`fill="url(#${k}L)" `, `fill="url(#${k}R)" `, `fill="url(#${k}T)" `])}</g>`;
    };
    return `<svg class="fg-crate" viewBox="14 30 132 126" aria-hidden="true" focusable="false">` +
      `<defs><linearGradient id="${k}T" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#5aa2ff"/><stop offset="1" stop-color="#2f7df5"/></linearGradient>` +
      `<linearGradient id="${k}L" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#1f74f2"/><stop offset="1" stop-color="#1663de"/></linearGradient>` +
      `<linearGradient id="${k}R" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#1159cf"/><stop offset="1" stop-color="#0b48ad"/></linearGradient>` +
      `<clipPath id="${k}C"><path d="M28 64 L80 38 L132 64 L132 119 L80 145 L28 119Z"/></clipPath></defs>` +
      `<ellipse class="sh" cx="80" cy="146" rx="50" ry="12"/><ellipse class="rp" cx="80" cy="140" rx="54" ry="16"/>` +
      `<g class="sl s1">${shell(64)}</g><g class="sl s2">${shell(44)}</g><g class="sl s3">${shell(24)}</g>` +
      `<g clip-path="url(#${k}C)"><path class="shn" d="M0 20 L26 20 L-14 170 L-40 170Z"/></g></svg>`;
  }
  const BITS = [[-108, -45, 10, 0, "accent"], [115, -30, 6, 0.25, "violet"], [-86, 38, 8, 0.5, "pink"], [101, 45, 12, 0.75, "accent"],
    [-43, -68, 6, 1, "green"], [65, -64, 9, 1.2, "violet"], [-122, 8, 7, 1.45, "accent"], [126, 4, 10, 0.1, "pink"],
    [-22, 71, 8, 0.6, "violet"], [29, 75, 6, 1.35, "accent"], [-68, -22, 12, 0.85, "accent"], [79, -8, 7, 1.6, "green"]];
  const bitsHtml = '<div class="fg-bits" aria-hidden="true">' + BITS.map(([x, y, w, d, c]) =>
    `<i style="--x:${x}px;--y:${y}px;--w:${w}px;--d:${d}s;--c:var(--${c})"></i>`).join("") + "</div>";
  qa(".forming-ghost").forEach((g) => {
    g.insertAdjacentHTML("beforeend", bitsHtml + crateSvg());
    g.dataset.ph = "plan";
    const svg = q(".fg-crate", g); svg.dataset.ph = "plan"; svg.classList.add("draw");
  });
  const buildGhost = $("formingGhost");
  const buildCrate = buildGhost ? q(".fg-crate", buildGhost) : null;
  const PH = ["plan", "write", "build", "pack"];
  let seenStart = null, writeAt = 0, lastPh = "";
  function setPh(ph) {
    if (ph === lastPh) return;
    lastPh = ph;
    buildCrate.dataset.ph = ph; buildGhost.dataset.ph = ph;
    if (ph !== "plan" && ph !== "write") qa(".sl", buildCrate).forEach((s) => s.classList.add("on"));
  }
  function crateTick() {
    if (!buildCrate) return;
    const view = $("stateBuilding");
    if (!view || view.classList.contains("hidden")) return;
    const st = app();
    const idx = st ? st.stageIndex : -1, started = st ? st.startedAt : null;
    if (started !== seenStart) {
      seenStart = started; lastPh = ""; writeAt = 0;
      qa(".sl", buildCrate).forEach((s) => s.classList.remove("on"));
      buildCrate.classList.remove("draw"); void buildCrate.getBoundingClientRect(); buildCrate.classList.add("draw");
    }
    const ph = PH[Math.max(0, Math.min(3, idx | 0))] || "plan";
    setPh(ph);
    // A change to an app that already exists: the app stays in view,
    // softened, while it is reworked -- the picture the finished card
    // already holds, not a new one.
    const prior = q("#thread .msg.vok") && q("#thread .msg.vlive");
    const src = prior && $("shot") ? $("shot").getAttribute("src") || "" : "";
    let face = q(".kr-face", buildGhost);
    if (src) {
      if (!face) { face = document.createElement("img"); face.className = "kr-face"; face.alt = ""; buildGhost.insertBefore(face, buildGhost.firstChild); }
      if (face.getAttribute("src") !== src) face.setAttribute("src", src);
      buildGhost.dataset.rev = "1";
    } else if (buildGhost.dataset.rev) {
      delete buildGhost.dataset.rev;
    }
    if (ph === "write") {
      if (!writeAt) writeAt = Date.now();
      const t = (Date.now() - writeAt) / 1000;
      qa(".sl", buildCrate).forEach((s, i) => { if (t >= [0, 25, 60][i]) s.classList.add("on"); });
    }
  }
  if (buildCrate) { crateTick(); setInterval(crateTick, 400); }

  // The stage glows softly while something is being made, and green once
  // it is done; the live line slides in when it changes.
  const stage = $("stage");
  function stageSync() {
    if (!stage) return;
    const s = ["Planning", "Building", "Done", "Failed", "Idle"].find((k) => { const e = $("state" + k); return e && !e.classList.contains("hidden"); });
    const v = s ? s.toLowerCase() : "idle";
    if (stage.dataset.kr !== v) stage.dataset.kr = v;
  }
  ["statePlanning", "stateBuilding", "stateDone", "stateFailed", "stateIdle"].forEach((id) => watch($(id), { attributes: true, attributeFilter: ["class"] }, stageSync));
  stageSync();
  const nowLine = $("nowLine");
  watch(nowLine, { childList: true, characterData: true, subtree: true }, () => {
    const box = $("peekBox"); if (!box) return;
    box.classList.remove("kr-stin"); void box.offsetWidth; box.classList.add("kr-stin");
  });

  /* ---- the finished app ---------------------------------------------------- */
  (function done() {
    const shotStage = q("#doneCard .shot-stage");
    if (shotStage) {
      const r = document.createElement("span");
      r.className = "kr-ready"; r.setAttribute("aria-hidden", "true");
      r.innerHTML = `<i>${ico("check", "")}</i>Ready`;
      shotStage.appendChild(r);
    }
    // What it may NOT do, beside what it may: the trust line's own words.
    const asks = $("asks"), capTrust = $("capTrust");
    function mirror() {
      if (!asks || !capTrust) return;
      const nots = capTrust.textContent.split(" · ").map((s) => s.trim()).filter((s) => /^cannot /.test(s));
      // The walls a person most wants to know about, when this app was not
      // given the door: the camera and the rest of their files.
      const st = app(), r = st && st.session && st.session.result;
      if (r && capTrust.textContent) {
        const caps = (r.asks || []).map(String);
        if (!caps.some((c) => c.startsWith("camera."))) nots.push("cannot use the camera");
        if (!caps.some((c) => c.startsWith("fs."))) nots.push("cannot open your other files");
      }
      const have = qa("li.no", asks).map((li) => li.textContent);
      if (have.length === nots.length && have.every((t, i) => t === nots[i])) return;
      qa("li.no", asks).forEach((li) => li.remove());
      nots.forEach((t) => { const li = document.createElement("li"); li.className = "no"; li.textContent = t; asks.appendChild(li); });
      qa("li", asks).forEach((li, i) => li.style.setProperty("--k", i));
    }
    // Where the file is, one click away, and the three systems it opens on.
    const meta = q("#doneCard .done-meta"), sub = q("#doneCard .done-sub"), size = $("doneSize");
    if (meta && sub && size) {
      sub.textContent = "";
      sub.append(size);
      sub.insertAdjacentHTML("beforeend", '<span class="kr-os-dots"><i>macOS</i><i>Windows</i><i>Linux</i></span>');
      const nm = $("doneName");
      const show = document.createElement("button");
      show.type = "button"; show.className = "kr-showf";
      show.innerHTML = ico(desktopApp() ? "folder" : "down") + `<span>${desktopApp() ? "Show in folder" : "Download"}</span>`;
      show.addEventListener("click", () => press($("filesSave")));
      if (nm) { const row = document.createElement("div"); row.className = "kr-dnrow"; nm.before(row); row.append(nm, show); }
    }
    watch(asks, { childList: true }, mirror);
    watch(capTrust, { childList: true, characterData: true, subtree: true }, mirror);
    mirror();
  })();

  /* ---- Your apps: a card to start a new one ---------------------------- */
  (function appsGrid() {
    const grid = $("appsGrid");
    if (!grid) return;
    const add = () => {
      if (q(".kr-newcard", grid) || !q(".app-card", grid)) return;
      const b = document.createElement("button");
      b.type = "button"; b.className = "kr-newcard";
      b.innerHTML = `<span class="kr-nth"><span><i>${ico("plus")}</i>New app</span></span>`;
      b.addEventListener("click", () => { const r = q('#side .side-row[data-side="home"]'); if (r) r.click(); setTimeout(() => { const t = $("homePrompt"); if (t) t.focus(); }, 300); });
      grid.insertBefore(b, grid.firstChild.classList && grid.firstChild.classList.contains("apps-finish") ? grid.firstChild.nextSibling : grid.firstChild);
      qa(".app-card", grid).forEach((c, i) => c.style.setProperty("--k", i + 1));
    };
    watch(grid, { childList: true }, add);
    add();
  })();

  /* ---- a toast: one line that says a thing happened ------------------- */
  const toastEl = document.createElement("div");
  toastEl.className = "kr-toast"; toastEl.setAttribute("role", "status");
  document.body.appendChild(toastEl);
  let toastT = 0;
  function toast(text, burst) {
    toastEl.innerHTML = `<i>${ico("check", "")}</i><span></span>` + (burst && !reduce ? '<span class="kr-burst" aria-hidden="true">' +
      Array.from({ length: 14 }, (_, i) => { const a = i / 14 * Math.PI * 2, d = 46 + (i % 3) * 14;
        return `<b style="--c:${["#3d6df0", "#22c55e", "#e8873f", "#7c5ce8", "#d6578f"][i % 5]};--x:${Math.cos(a) * d}px;--y:${Math.sin(a) * d}px;animation-delay:${i * 8}ms"></b>`; }).join("") + "</span>" : "");
    q("span", toastEl).textContent = text;
    toastEl.classList.add("on");
    clearTimeout(toastT); toastT = setTimeout(() => toastEl.classList.remove("on"), 2600);
  }
  // ide.js says what it built with the same toast.
  window.krToast = toast;
  // Published: the sheet closes and the link is on the clipboard. Say so
  // where it is seen. Only when a NEW link appeared, so closing the sheet
  // any other way says nothing.
  let linking = false; // Share's Make a link publishes too, and says so itself
  (function published() {
    const sheet = $("publishSheet");
    if (!sheet) return;
    let before = null;
    const link = () => { const s = app(); return s && s.session && s.session.result ? s.session.result.share_url || null : null; };
    watch(sheet, { attributes: true, attributeFilter: ["class"] }, () => {
      if (!sheet.classList.contains("hidden")) { before = link(); return; }
      const now = link();
      if (now && now !== before && !linking) toast("Published. The link is copied", true);
      before = now;
    });
  })();

  /* Press a card, then a button that appears once its page is open. */
  function thenPress(id, ms = 6000) {
    const t0 = Date.now();
    const tick = () => {
      const b = $(id);
      if (b && !b.classList.contains("hidden") && b.offsetParent) { press(b); return; }
      if (Date.now() - t0 < ms) setTimeout(tick, 100);
    };
    setTimeout(tick, 150);
  }

  /* ---- Your apps: Run and Share on a card, on hover ------------------- */
  (function appHover() {
    const grid = $("appsGrid");
    if (!grid) return;
    const add = () => qa(".app-card", grid).forEach((card) => {
      const well = q(".thumb-well", card);
      if (!well || q(".kr-hov", card) || well.classList.contains("blank")) return;
      const h = document.createElement("span");
      h.className = "kr-hov";
      h.innerHTML = `<span class="kr-hb dark" role="button" tabindex="0" data-a="run">${ico("play")}Run</span>` +
        `<span class="kr-hb" role="button" tabindex="0" data-a="share">${ico("share")}Share</span>`;
      h.addEventListener("click", (e) => {
        const b = e.target.closest("[data-a]"); if (!b) return;
        e.stopPropagation(); e.preventDefault();
        card.click();
        thenPress(b.dataset.a === "run" ? "barRun" : "barShare");
      });
      well.appendChild(h);
    });
    watch(grid, { childList: true }, add);
    add();
  })();

  /* ---- the Gallery: a thumb under the category, and windows round apps -- */
  (function gallery() {
    const cats = $("cloudCats");
    if (cats) {
      const th = document.createElement("span"); th.className = "kr-thumb dark"; th.setAttribute("aria-hidden", "true");
      const place = () => {
        if (!th.isConnected) cats.insertBefore(th, cats.firstChild);
        const on = q(".cat.on", cats);
        if (!on || !on.offsetWidth) { th.style.opacity = 0; return; }
        th.style.opacity = 1; th.style.left = on.offsetLeft + "px"; th.style.width = on.offsetWidth + "px";
      };
      watch(cats, { childList: true, subtree: true, attributes: true, attributeFilter: ["class"] }, () => requestAnimationFrame(place));
      addEventListener("resize", place);
    }
    const grid = $("cloudGrid");
    const frame = () => {
      qa(".cloud-card", grid || document).forEach((c, i) => {
        c.style.setProperty("--k", i % 12);
        const shot = q(".cloud-shot", c);
        if (!shot || q(".kr-fb", shot)) return;
        const name = (q(".cloud-name", c) || {}).textContent || "";
        const fb = document.createElement("span"); fb.className = "kr-fb"; fb.setAttribute("aria-hidden", "true");
        fb.innerHTML = `<span class="kr-lights"><i></i><i></i><i></i></span><span class="kr-fbn"></span>`;
        q(".kr-fbn", fb).textContent = name;
        shot.insertBefore(fb, shot.firstChild);
        // Remix: start your own from what this one is, in your words.
        const desc = (q(".cloud-desc", c) || {}).textContent || "";
        if (!desc) return;
        const rx = document.createElement("span");
        rx.className = "kr-hov";
        rx.innerHTML = `<span class="kr-hb dark" role="button" tabindex="0">${ico("spark")}Remix</span>`;
        rx.addEventListener("click", (e) => { e.stopPropagation(); e.preventDefault(); remix(desc, name); });
        shot.appendChild(rx);
      });
    };
    watch(grid, { childList: true, subtree: true }, frame);
  })();
  function remix(desc, name) {
    const home = q('#side .side-row[data-side="home"]');
    if (home) home.click();
    setTimeout(() => {
      setMode("create");
      const ta = $("homePrompt"); if (!ta) return;
      const s = desc.replace(/[.\s]+$/, "").replace(/^\w/, (c) => c.toUpperCase()) + ", but ";
      ta.value = ""; ta.focus();
      let k = 0;
      const step = () => { ta.value = s.slice(0, ++k); ta.dispatchEvent(new Event("input")); if (k < s.length) setTimeout(step, reduce ? 0 : 10); };
      step();
      toast(`Starting from ${name}. Say what to change`);
    }, 250);
  }

  /* ---- one gallery app: the app in a window, everything else beside it -- */
  (function detail() {
    const stageEl = q("#viewApp .appstage"), ident = q("#viewApp .ident"), bar2 = q("#viewApp .actionbar");
    if (!stageEl || !ident || !bar2) return;
    bar2.parentNode.insertBefore(ident, bar2);
    const fb = document.createElement("span");
    fb.className = "kr-fb"; fb.setAttribute("aria-hidden", "true");
    fb.innerHTML = '<span class="kr-lights"><i></i><i></i><i></i></span><span class="kr-fbn"></span>';
    stageEl.insertBefore(fb, stageEl.firstChild);
    const nm = $("detailName");
    const paint = () => { q(".kr-fbn", fb).textContent = nm ? nm.textContent : ""; };
    watch(nm, { childList: true, characterData: true, subtree: true }, paint);
    paint();
    const view = $("viewApp");
    watch(view, { attributes: true, attributeFilter: ["class"] }, () => { if (!view.classList.contains("hidden")) { const m = q(".detail", view); if (m) m.scrollTop = 0; } });
  })();

  /* ---- Settings: an indicator that slides to the section ---------------- */
  (function settingsNav() {
    const nav2 = $("setNav");
    if (!nav2) return;
    const th = document.createElement("span"); th.className = "kr-nind"; th.setAttribute("aria-hidden", "true");
    const place = () => {
      if (!th.isConnected) nav2.insertBefore(th, nav2.firstChild);
      const on = q("button.on", nav2);
      if (!on || !on.offsetHeight) { th.style.opacity = 0; return; }
      th.style.opacity = 1; th.style.height = on.offsetHeight + "px"; th.style.transform = `translateY(${on.offsetTop}px)`;
    };
    // An icon beside each section, as in the design. The list is built by
    // app.js from the section headings, so they are added as it appears.
    const ICONS = { "your ai": "cpu", agent: "cpu", output: "folder", updates: "refresh", gallery: "compass", support: "msg", privacy: "shield",
      profile: "user", appearance: "paint", "sign in with": "key", account: "user", keys: "key", terminal: "term" };
    const iconify = () => qa("button", nav2).forEach((b) => {
      if (q(".kr-ico", b)) return;
      const k = b.textContent.trim().toLowerCase();
      b.insertAdjacentHTML("afterbegin", ico(ICONS[k] || "gear"));
    });
    watch(nav2, { childList: true }, iconify);
    iconify();
    watch(nav2, { childList: true, subtree: true, attributes: true, attributeFilter: ["class"] }, () => requestAnimationFrame(place));
    watch($("setSheet"), { attributes: true, attributeFilter: ["class"] }, () => requestAnimationFrame(place));
  })();

  /* ---- Settings: Your AI as cards, and Reduce motion -------------------- */
  (function settingsAi() {
    const box = $("setAgentCards"), sheetEl = $("setSheet");
    if (!box || !sheetEl) return;
    const API = new Set(["anthropic", "openai"]);
    const sheet = () => { try { openAiSheet(); } catch (e) {} };
    function paint() {
      const st = app(); if (!st) return;
      const list = (st.agents || []).filter((a) => !API.has(a.name));
      const order = [...list.filter((a) => a.state === "working"), ...list.filter((a) => a.state !== "working")];
      const sig = JSON.stringify([st.agent, order.map((a) => [a.name, a.state, a.remedy || ""])]);
      if (box.dataset.sig === sig) return;
      box.dataset.sig = sig;
      box.innerHTML = "";
      if (!order.length) { box.innerHTML = `<p class="kr-agc-empty">${st.agentsError ? "Krate could not look for AI tools on this computer." : "Looking for AI tools on this computer…"}</p>`; return; }
      order.forEach((a, i) => {
        const ok = a.state === "working", on = ok && a.name === st.agent;
        const b = document.createElement("div");
        b.className = "kr-agc" + (on ? " on" : "") + (ok ? "" : " off"); b.setAttribute("role", "listitem");
        b.style.setProperty("--k", i);
        const sub = ok ? (on ? "Writing your apps" : "Ready to use")
          : a.state === "missing" ? "Not installed" : a.state === "paused" ? "Paused just now"
          : /sign/i.test(a.detail || "") ? "Installed, needs a sign-in" : "Installed, needs a one-time fix";
        b.innerHTML = `<span class="kr-lgb">${logo(a.name)}</span><span class="kr-agt"><b></b><small></small>${a.state === "missing" && a.remedy ? '<span class="kr-cmdl"><code></code><button type="button" data-copy>Copy</button></span>' : ""}</span>` +
          (ok ? `<span class="kr-stt ok">${on ? "In use" : "Ready"}</span>` : `<button type="button" class="kr-stt act">${a.state === "missing" ? "Install" : "Fix"}</button>`);
        q("b", b).textContent = a.label || a.name;
        q("small", b).textContent = sub;
        const code = q("code", b); if (code) code.textContent = a.remedy;
        const cp = q("[data-copy]", b);
        if (cp) cp.addEventListener("click", (e) => { e.stopPropagation(); (navigator.clipboard ? navigator.clipboard.writeText(a.remedy) : Promise.reject()).then(() => { cp.textContent = "Copied"; setTimeout(() => { cp.textContent = "Copy"; }, 1200); }, () => {}); });
        const act = q("button.kr-stt", b); if (act) act.addEventListener("click", (e) => { e.stopPropagation(); sheet(); });
        if (ok && !on) {
          b.tabIndex = 0; b.classList.add("pick"); b.title = `Use ${a.label || a.name}`;
          const pick = async () => { try { await useAgent(a.name); toast(`${a.label || a.name} will write your apps`); } catch (e) { sheet(); } paint(); };
          b.addEventListener("click", pick);
          b.addEventListener("keydown", (e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); pick(); } });
        }
        box.appendChild(b);
      });
    }
    watch(sheetEl, { attributes: true, attributeFilter: ["class"] }, () => { if (!sheetEl.classList.contains("hidden")) paint(); });
    setInterval(() => { if (!sheetEl.classList.contains("hidden")) paint(); }, 1000);
    // The key row opens the sheet that holds the keys. (app.js pressed the
    // AI chip here, which now opens a menu beside a chip the dialog hides.)
    const keyBtn = $("setAgentBtn");
    if (keyBtn) keyBtn.addEventListener("click", (e) => { e.stopImmediatePropagation(); sheet(); }, true);
    if (!desktopApp()) { const kp = $("setKeyPanel"); if (kp) kp.hidden = true; }
    // Reduce motion: this computer only, on top of the system's own setting.
    const calm = $("setCalm");
    let on = false; try { on = localStorage.getItem("krate-calm") === "1"; } catch (e) {}
    const apply = (v) => { document.body.classList.toggle("kr-calm", v); if (calm) { calm.classList.toggle("on", v); calm.setAttribute("aria-checked", String(v)); } };
    apply(on);
    if (calm) calm.addEventListener("click", () => {
      const v = calm.classList.contains("on");
      try { localStorage.setItem("krate-calm", v ? "1" : "0"); } catch (e) {}
      apply(v);
    });
  })();

  /* ---- first run: which step this is ------------------------------------ */
  const ob = $("viewOnboard");
  if (ob) {
    const scenes = qa(".ob-scene", ob);
    const steps = scenes.map((s) => s.dataset.step).filter((n) => desktopApp() || n !== "2");
    const dots = document.createElement("div");
    dots.className = "ob-dots"; dots.setAttribute("aria-hidden", "true");
    dots.innerHTML = steps.map(() => "<i></i>").join("");
    ob.appendChild(dots);
    let last = -1;
    const paint = () => {
      const on = scenes.find((s) => s.classList.contains("on"));
      const at = on ? steps.indexOf(on.dataset.step) : 0;
      qa("i", dots).forEach((d, i) => d.classList.toggle("on", i === at));
      if (on) on.classList.toggle("kr-back", at < last);
      last = at;
    };
    scenes.forEach((s) => watch(s, { attributes: true, attributeFilter: ["class"] }, paint));
    paint();
    qa(".fan-card", ob).forEach((c, i) => c.style.setProperty("--k", i));
  }

  /* ---- Share: one sheet with the three ways ------------------------------ */
  // Replaces app.js's small Share menu. Every way still runs app.js's own
  // code: a link is app.js's publish (unlisted), the file is its card and
  // its gift-for-a-new-person, Publish is its publish sheet. A function
  // declared at the top of app.js is a property of the page, so assigning
  // it here changes every door that opens Share.
  const shareWrap = document.createElement("div");
  shareWrap.className = "sheet-wrap hidden kr-share-wrap"; shareWrap.id = "krShare";
  shareWrap.innerHTML = `<div class="sheet kr-share" role="dialog" aria-modal="true" aria-labelledby="krShareH">
    <div class="kr-sh-h"><div><h2 id="krShareH">Share <span data-n></span></h2><p class="sheet-sub">One file. Whoever gets it double-clicks, and it opens.</p></div>
      <button type="button" class="kr-x" data-close title="Close" aria-label="Close">${ico("x")}</button></div>
    <div class="kr-shfile"><span class="kr-shic"><img src="krate-logo.png" alt=""></span><span class="kr-sht"><b data-f></b><small data-s></small></span><em data-v hidden></em></div>
    <div class="kr-ways" role="tablist">
      <button type="button" class="kr-way" data-w="link" role="tab" style="--c:#3d6df0"><i>${ico("link")}</i><b>Share a link</b><small>Anyone with it can open the app</small></button>
      <button type="button" class="kr-way" data-w="file" role="tab" style="--c:#7c5ce8"><i>${ico("file")}</i><b>Send the file</b><small>Email, a chat, a USB stick</small></button>
      <button type="button" class="kr-way" data-w="pub" role="tab" style="--c:#22a35a"><i>${ico("globe")}</i><b>Publish</b><small>List it in the public gallery</small></button>
    </div>
    <div class="kr-wp" data-w="link"></div>
    <div class="kr-wp" data-w="file"></div>
    <div class="kr-wp" data-w="pub"></div>
    <p class="kr-shnote">${ico("info")}<span>New to Krate? They install it once, like a video player.</span></p>
  </div>`;
  document.body.appendChild(shareWrap);
  let shareApp = null, shareVer = null;
  const linkOf = () => { const s = app(); const r = s && s.session && s.session.result; return r && r.share_url ? r.share_url : null; };
  const isCurrent = () => { try { const c = currentApp(); return !!(c && shareApp && c.path === shareApp.path); } catch (e) { return false; } };
  function closeShare() { shareWrap.classList.add("hidden"); }
  shareWrap.addEventListener("click", (e) => {
    if (e.target === shareWrap || e.target.closest("[data-close]")) closeShare();
    const w = e.target.closest(".kr-way"); if (w) pickWay(w.dataset.w);
  });
  document.addEventListener("keydown", (e) => { if (e.key === "Escape" && !shareWrap.classList.contains("hidden")) { closeShare(); e.stopPropagation(); } }, true);
  function pickWay(w) {
    qa(".kr-way", shareWrap).forEach((b) => { b.classList.toggle("on", b.dataset.w === w); b.setAttribute("aria-selected", String(b.dataset.w === w)); });
    qa(".kr-wp", shareWrap).forEach((p) => p.classList.toggle("on", p.dataset.w === w));
  }
  function paintLink(url, fresh) {
    const pane = q('.kr-wp[data-w="link"]', shareWrap);
    const shown = url.replace(/^https?:\/\//, "");
    let qr = ""; try { qr = window.krQrSvg ? window.krQrSvg(url, "kr-qrsvg") : ""; } catch (e) {}
    pane.innerHTML = `<div class="kr-linkf"><span class="kr-u"></span><button type="button" class="btn kr-dark" data-copy>${ico("copy")}<span>Copy</span></button></div>` +
      (qr ? `<div class="kr-qr">${qr}<p>Point a phone at this to send it on, or paste the link anywhere: a chat, an email, a post.<br><span>Only people with the link can find it.</span></p></div>` : "");
    const u = q(".kr-u", pane);
    if (fresh && !reduce) [...shown].forEach((ch, i) => { const c = document.createElement("span"); c.className = "kr-c"; c.style.animationDelay = i * 14 + "ms"; c.textContent = ch; u.appendChild(c); });
    else u.textContent = shown;
    q("[data-copy]", pane).addEventListener("click", (e) => {
      const b = e.currentTarget;
      const done = () => { b.classList.add("ok"); q("span", b).textContent = "Copied"; setTimeout(() => { b.classList.remove("ok"); q("span", b).textContent = "Copy"; }, 1400); };
      (navigator.clipboard ? navigator.clipboard.writeText(url) : Promise.reject()).then(done, done);
    });
  }
  function paintMakeLink() {
    const pane = q('.kr-wp[data-w="link"]', shareWrap);
    if (!isCurrent()) {
      pane.innerHTML = `<p class="kr-shp">A link is made for the newest version. Open it and share from there.</p>`;
      return;
    }
    pane.innerHTML = `<button type="button" class="btn kr-dark kr-make"><span class="kr-pg"></span><span class="kr-lb">Make a link</span></button><p class="kr-sherr" hidden></p>`;
    const b = q(".kr-make", pane);
    b.addEventListener("click", async () => {
      if (b.dataset.busy) return;
      b.dataset.busy = "1"; b.classList.add("busy");
      q(".kr-lb", b).textContent = `Uploading ${(shareApp && shareApp.size) || "the app"}…`;
      const sheet = $("publishSheet");
      linking = true;
      try {
        openPublishSheet();
        if (sheet) sheet.classList.add("hidden");
        const listed = $("pubListed"); if (listed) listed.checked = false;
        await publishFromSheet();
      } catch (e) { /* app.js says why in the publish sheet's note */ }
      linking = false;
      const url = linkOf();
      if (url) { paintLink(url, true); toast("Link made and copied", true); return; }
      // Publishing needs an account: the publish sheet carries that step.
      if ($("pubSignin") && !$("pubSignin").classList.contains("hidden")) { closeShare(); if (sheet) sheet.classList.remove("hidden"); return; }
      delete b.dataset.busy; b.classList.remove("busy"); q(".kr-lb", b).textContent = "Make a link";
      const why = (($("pubNote") || {}).textContent || "").trim();
      const err = q(".kr-sherr", pane); err.textContent = why || "The link could not be made just now. Try again in a moment."; err.hidden = false;
    });
  }
  function paintFile() {
    const pane = q('.kr-wp[data-w="file"]', shareWrap);
    if (!desktopApp()) {
      pane.innerHTML = `<p class="kr-shp">Download the file and send it any way you like. Whoever gets it opens it with Krate.</p>
        <div class="kr-shacts"><button type="button" class="btn kr-dark" data-dl>${ico("down")}Download the file</button></div>`;
      q("[data-dl]", pane).addEventListener("click", () => press($("filesSave")));
      return;
    }
    pane.innerHTML = `<div class="kr-shacts"><button type="button" class="btn kr-dark" data-send>${ico("share")}<span>Send it</span></button>
        <button type="button" class="btn kr-ghost" data-show>${ico("folder")}Show in folder</button></div>
      <p class="kr-shst" data-st>A card with the app inside: Mail, Messages and AirDrop are one click away.</p>
      <div class="kr-new"><b>For someone new to Krate</b><small>A file that installs Krate once, then opens the app.</small>
        <div class="kr-os"><button type="button" class="btn kr-ghost" data-os="mac">Mac</button><button type="button" class="btn kr-ghost" data-os="windows">Windows</button><button type="button" class="btn kr-ghost" data-os="linux">Linux</button></div>
        <p class="kr-shst" data-wst></p></div>`;
    const st = q("[data-st]", pane), wst = q("[data-wst]", pane);
    q("[data-send]", pane).addEventListener("click", () => { try { sendCard(shareApp, st); } catch (e) {} });
    q("[data-show]", pane).addEventListener("click", () => { if (isCurrent()) press($("filesSave")); else try { invokeReveal(); } catch (e) {} });
    qa("[data-os]", pane).forEach((b) => b.addEventListener("click", () => { try { makeWrap(shareApp, b.dataset.os, wst); } catch (e) {} }));
  }
  const invokeReveal = () => press($("filesSave"));
  function paintPub() {
    const pane = q('.kr-wp[data-w="pub"]', shareWrap);
    pane.innerHTML = `<p class="kr-shp">It goes in the public gallery under your name, with a picture of it running and one line about it. Publish again later to change any of that.</p>
      <div class="kr-shacts"><button type="button" class="btn kr-dark" data-pub>${ico("globe")}Publish…</button></div>`;
    q("[data-pub]", pane).addEventListener("click", () => {
      closeShare();
      try { openPublishSheet(); const l = $("pubListed"); if (l) l.checked = true; } catch (e) {}
    });
    if (!isCurrent()) pane.innerHTML = `<p class="kr-shp">Publishing puts the newest version in the gallery. Open it and publish from there.</p>`;
  }
  function openShare(which, version) {
    let cur = null; try { cur = currentApp(); } catch (e) {}
    shareApp = which && which.path ? which : cur;
    if (!shareApp) return;
    shareVer = version || null;
    const s = app();
    const name = (shareApp.name || "").replace(/\.krate$/, "").replace(/[-_]+/g, " ").replace(/\b\w/g, (c) => c.toUpperCase()) || "your app";
    q("[data-n]", shareWrap).textContent = name;
    q("[data-f]", shareWrap).textContent = shareApp.name || "app.krate";
    q("[data-s]", shareWrap).textContent = [shareApp.size, "opens on macOS, Windows and Linux"].filter(Boolean).join(" · ");
    const builds = s && s.session && s.session.builds;
    const v = q("[data-v]", shareWrap);
    if (version && builds && version < builds) { v.hidden = false; v.textContent = `v${version} · newest is v${builds}`; } else v.hidden = true;
    const url = isCurrent() ? linkOf() : null;
    if (url) paintLink(url, false); else paintMakeLink();
    paintFile(); paintPub();
    pickWay("link");
    shareWrap.classList.remove("hidden");
    setTimeout(() => { const b = q(".kr-way.on", shareWrap); if (b) b.focus(); }, 60);
  }
  try { if (typeof openSendSheet === "function") window.openSendSheet = (which, version) => openShare(which, version); } catch (e) {}

  /* ---- first run: a welcome over Home, and a three-step tour ------------- */
  // The old welcome was three full screens before the app. Now Home is
  // there from the first second, with a small card over it: Skip, or a tour
  // that points at the real box, the real AI chip and the real sidebar.
  // app.js still decides WHEN (needsOnboarding) and records that it was
  // seen (markOnboarded); this only changes what "the welcome" is.
  const obWrap = document.createElement("div");
  obWrap.className = "kr-ob"; obWrap.id = "krOb";
  obWrap.setAttribute("role", "dialog"); obWrap.setAttribute("aria-modal", "true"); obWrap.setAttribute("aria-labelledby", "krObT");
  obWrap.innerHTML = `<div class="kr-ob-card" tabindex="-1">
    <button type="button" class="kr-x kr-ob-x" aria-label="Close">${ico("x")}</button>
    <img class="kr-ob-mk" src="krate-logo.png" alt="">
    <h2 id="krObT">Say what you want.<br>Get an app.</h2>
    <p class="kr-ob-s">Take a quick tour to see how Krate Studio works, or close this and start making.</p>
    <div class="kr-pv" data-ph="a" aria-hidden="true"><div class="kr-pv-win">
      <div class="kr-pv-side"><b></b><i class="on"></i><i></i><i></i><i></i><span></span><span></span><span></span></div>
      <div class="kr-pv-main">
        <div class="kr-pv-a"><p class="kr-pv-q">What are we <em>building</em>?</p><div class="kr-pv-cmp"><span class="kr-pv-tx"></span><span class="kr-pv-car"></span><span class="kr-pv-go">${ico("up")}</span></div></div>
        <div class="kr-pv-b"><div class="kr-pv-card"><div class="kr-pv-th"><svg viewBox="14 30 132 126">${[1, 2, 3].map((n) => { const y = [0, 104, 84, 64][n]; return `<g class="kr-sl kr-s${n}"><path class="l" d="M28 ${y} L80 ${y + 26} L80 ${y + 41} L28 ${y + 15}Z"/><path class="r" d="M80 ${y + 26} L132 ${y} L132 ${y + 15} L80 ${y + 41}Z"/><path class="t" d="M28 ${y} L80 ${y - 26} L132 ${y} L80 ${y + 26}Z"/></g>`; }).join("")}</svg></div>
          <div class="kr-pv-nm"><b>Weather</b><small>Writing the app</small></div></div></div>
        <div class="kr-pv-c"><div class="kr-pv-app"><img src="cards/card3.jpg" alt=""></div><div class="kr-pv-file"><img src="krate-doc.png" alt=""><b>weather.krate</b><small>opens on every desktop</small></div></div>
      </div></div></div>
    <div class="kr-ob-f"><button type="button" class="kr-plain kr-ob-skip">Skip</button><span class="kr-dots"><i class="on"></i><i></i><i></i><i></i></span><button type="button" class="kr-dark kr-ob-go">Take the tour</button></div>
  </div>`;
  document.body.appendChild(obWrap);
  const tourEl = document.createElement("div");
  tourEl.className = "kr-tour"; tourEl.setAttribute("aria-live", "polite");
  tourEl.innerHTML = `<div class="kr-tr-hole"></div><div class="kr-tr-card" role="dialog" aria-labelledby="krTrT" tabindex="-1"><span class="kr-tr-arr"></span>
    <div class="kr-tr-top"><span class="kr-tr-n"></span><button type="button" class="kr-x" aria-label="End the tour">${ico("x")}</button></div>
    <h4 id="krTrT"></h4><p></p>
    <div class="kr-tr-f"><span class="kr-dots"><i></i><i></i><i></i></span><button type="button" class="kr-plain kr-tr-back">Back</button><button type="button" class="kr-dark kr-tr-next">Next</button></div></div>`;
  document.body.appendChild(tourEl);
  const homeBox = () => $("homePrompt");
  let pvT = 0, pvRun = 0;
  const sleep = (ms) => new Promise((r) => { pvT = setTimeout(r, ms); });
  async function pvPlay() {
    const my = ++pvRun, pv = q(".kr-pv", obWrap), tx = q(".kr-pv-tx", obWrap), s = "a weather app for my cities";
    if (reduce) { pv.dataset.ph = "c"; return; }
    while (my === pvRun && obWrap.classList.contains("on")) {
      pv.dataset.ph = "a"; tx.textContent = "";
      for (let i = 1; i <= s.length; i++) { tx.textContent = s.slice(0, i); await sleep(42); if (my !== pvRun) return; }
      await sleep(500); if (my !== pvRun) return;
      pv.dataset.ph = "b"; await sleep(2600); if (my !== pvRun) return;
      pv.dataset.ph = "c"; await sleep(3000);
    }
  }
  // On the web a note about what is free also opens on a first visit.
  // One thing at a time: it waits until the welcome and the tour are done.
  let heldNote = null;
  const holdNote = () => { const n = $("welcomeSheet"); if (n && !n.classList.contains("hidden")) { n.classList.add("hidden"); heldNote = n; } };
  const releaseNote = () => { if (heldNote) { heldNote.classList.remove("hidden"); heldNote = null; return true; } return false; };
  function obOpen() {
    closePop();
    obWrap.classList.remove("out"); obWrap.classList.add("on");
    holdNote(); setTimeout(holdNote, 600);
    pvPlay();
    setTimeout(() => { const c = q(".kr-ob-card", obWrap); if (c) c.focus({ preventScroll: true }); }, 400);
  }
  function obClose(tour) {
    if (!obWrap.classList.contains("on")) return;
    try { markOnboarded(); } catch (e) {}
    obWrap.classList.add("out"); pvRun++; clearTimeout(pvT);
    setTimeout(() => {
      obWrap.classList.remove("on", "out");
      if (tour) tourStart(); else if (!releaseNote()) { const b = homeBox(); if (b) b.focus(); }
    }, reduce ? 0 : 280);
  }
  q(".kr-ob-x", obWrap).addEventListener("click", () => obClose(false));
  q(".kr-ob-skip", obWrap).addEventListener("click", () => obClose(false));
  q(".kr-ob-go", obWrap).addEventListener("click", () => obClose(true));
  obWrap.addEventListener("click", (e) => { if (e.target === obWrap) obClose(false); });

  const firstVisible = (...els) => els.find((el) => el && el.offsetParent !== null && el.getBoundingClientRect().width > 0) || null;
  const TOUR = [
    { el: () => firstVisible(q("#viewHome .composer-shell .bigbar"), q("#viewHome .composer-shell")), t: "Say what you want",
      p: () => desktopApp() ? "Describe an app in a sentence, like “a habit tracker with streaks”. Your AI writes it, and Krate builds it, opens it and checks it." : "Describe an app in a sentence, like “a habit tracker with streaks”. Krate’s AI writes it, builds it, opens it and checks it." },
    { el: () => firstVisible($("builtByChip"), $("agentChip")), t: () => desktopApp() ? "Use the AI you already have" : "Krate’s AI does the writing",
      p: () => desktopApp() ? "Claude, Codex, Gemini and others. Krate works with whichever one you pick, and you can change it here at any time." : "Here it is Krate’s own. In Studio on your computer you can pick Claude, Codex or Gemini instead." },
    { el: () => firstVisible(q("#side .side-nav")), can: () => !!q("#side .side-nav"), side: "right", before: () => openSide(true), t: "Every app is one small file",
      p: "What you make lands in Your apps as a single .krate file you can send to anyone. Find more, or remix one, in the Gallery." },
  ];
  let trI = -1, sideOpened = false;
  // On a narrow window the sidebar is a drawer: it opens for the step that
  // points at it, and closes again when the tour is over.
  function openSide(on) {
    const side = $("side"), t = $("sideToggle");
    if (!side || !t || wide()) return;
    const open = side.dataset.open === "true";
    if (on && !open) { press(t); sideOpened = true; return true; }
    if (!on && open && sideOpened) { press(t); sideOpened = false; }
    return false;
  }
  // Only the steps this window can show: a phone's composer has no AI chip.
  let steps = TOUR;
  function tourStart() {
    steps = TOUR.filter((t) => (t.can ? t.can() : !!t.el()));
    if (!steps.length) return;
    q(".kr-tr-f .kr-dots", tourEl).innerHTML = steps.map(() => "<i></i>").join("");
    trI = 0; sideOpened = false; tourEl.classList.add("on"); tourShow();
  }
  function tourEnd(done) {
    if (trI < 0) return;
    trI = -1; tourEl.classList.remove("on");
    openSide(false);
    if (releaseNote()) return;
    if (done) toast("You are all set. Say what you want to make.");
    setTimeout(() => { const b = homeBox(); if (b) b.focus(); }, 200);
  }
  function tourShow() {
    const st = steps[trI];
    // Wait for a drawer that just started opening to finish moving.
    if (st.before) { if (st.before()) return setTimeout(() => { if (trI >= 0) tourShow(); }, 380); }
    else openSide(false);
    const el = st.el();
    if (!el) { if (trI < steps.length - 1) { trI++; return tourShow(); } return tourEnd(true); }
    const r = el.getBoundingClientRect(), pad = 8, hole = q(".kr-tr-hole", tourEl), card = q(".kr-tr-card", tourEl);
    const rad = parseFloat(getComputedStyle(el).borderTopLeftRadius) || 12;
    Object.assign(hole.style, { left: r.left - pad + "px", top: r.top - pad + "px", width: r.width + pad * 2 + "px", height: r.height + pad * 2 + "px", borderRadius: rad + pad + "px" });
    q(".kr-tr-n", tourEl).textContent = `${trI + 1} of ${steps.length}`;
    q("h4", tourEl).textContent = typeof st.t === "function" ? st.t() : st.t;
    q("p", card).textContent = typeof st.p === "function" ? st.p() : st.p;
    qa(".kr-dots i", card).forEach((d, i) => d.classList.toggle("on", i === trI));
    q(".kr-tr-back", tourEl).style.visibility = trI ? "visible" : "hidden";
    q(".kr-tr-next", tourEl).textContent = trI === steps.length - 1 ? "Start making" : "Next";
    card.classList.remove("in"); void card.offsetWidth;
    const cw = Math.min(320, innerWidth - 32), ch = card.offsetHeight || 190;
    let x, y, side = st.side || "below";
    if (side === "right") { x = r.right + 20; y = Math.max(16, Math.min(r.top + 10, innerHeight - ch - 16)); }
    if (side === "right" && x + cw > innerWidth - 16) side = "below";
    if (side !== "right") {
      x = Math.min(Math.max(16, r.left + r.width / 2 - cw / 2), innerWidth - cw - 16); y = r.bottom + 18; side = "below";
      if (y + ch > innerHeight - 16) { y = Math.max(16, r.top - ch - 18); side = "above"; }
    }
    Object.assign(card.style, { left: x + "px", top: y + "px", width: cw + "px" }); card.dataset.side = side;
    const arr = q(".kr-tr-arr", tourEl);
    if (side === "right") { arr.style.left = "-6px"; arr.style.top = Math.min(ch - 24, Math.max(18, r.top + Math.min(r.height, 40) / 2 - y)) + "px"; }
    else { arr.style.top = ""; arr.style.left = Math.min(cw - 24, Math.max(18, r.left + r.width / 2 - x - 6)) + "px"; }
    card.classList.add("in");
    card.focus({ preventScroll: true });
  }
  q(".kr-tr-next", tourEl).addEventListener("click", () => { if (trI >= steps.length - 1) return tourEnd(true); trI++; tourShow(); });
  q(".kr-tr-back", tourEl).addEventListener("click", () => { if (trI > 0) { trI--; tourShow(); } });
  q(".kr-tr-top .kr-x", tourEl).addEventListener("click", () => tourEnd(false));
  addEventListener("resize", () => { if (trI >= 0) tourShow(); });
  document.addEventListener("keydown", (e) => {
    if (obWrap.classList.contains("on") && e.key === "Escape") { e.stopPropagation(); e.preventDefault(); obClose(false); return; }
    if (trI < 0) return;
    if (e.key === "Escape") { e.stopPropagation(); e.preventDefault(); tourEnd(false); }
    else if (e.key === "ArrowRight" || (e.key === "Enter" && !e.target.closest(".kr-tr-back, .kr-tr-top .kr-x"))) { e.preventDefault(); q(".kr-tr-next", tourEl).click(); }
    else if (e.key === "ArrowLeft") { e.preventDefault(); q(".kr-tr-back", tourEl).click(); }
  }, true);
  // app.js shows its welcome with showView("onboard"): on first run, and
  // from Settings' replay. Both now land on Home with the card over it.
  try {
    if (typeof showView === "function") {
      const show = showView;
      window.showView = function (name, ...rest) {
        if (name === "onboard") {
          try { closeSettings(); } catch (e) {}
          try { enterHome(); } catch (e) { show("home"); }
          setTimeout(obOpen, 350);
          return;
        }
        return show.call(this, name, ...rest);
      };
      // The welcome may already be up: an answer that comes back at once
      // (the browsable mock, a cached web session) lets app.js show it
      // before this file has loaded.
      const old = $("viewOnboard");
      if (old && !old.classList.contains("hidden")) window.showView("onboard");
    }
  } catch (e) {}

  /* ---- the sidebar's Port an app ---------------------------------------- */
  const sidePort = $("sidePort");
  if (sidePort) sidePort.addEventListener("click", () => {
    const homeRow = q('#side .side-row[data-side="home"]');
    if (homeRow && $("viewHome") && $("viewHome").classList.contains("hidden")) homeRow.click();
    setMode("port");
    startPort();
  });
})();
