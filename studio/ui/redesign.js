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
      // `keep`: a row that changes in place (a remove that asks twice) runs
      // with the menu still open; every other row closes it first, so a row
      // may open a menu of its own.
      b.addEventListener("click", (e) => { e.stopPropagation(); if (it.keep) { if (it.run(b) !== true) closePop(); return; } closePop(); it.run && it.run(); });
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

  /* ---- Studio opening -------------------------------------------------- */
  // The desktop: the mark's three layers drop in and stack, bottom first,
  // while a hairline fills underneath. The web: the mark breathes beside
  // the name and a short bar runs through the five colours; after 3 s a
  // line says why. It leaves once a screen is really there, and not before
  // the layers have landed.
  (function boot() {
    if (reduce) return;
    const web = !desktopApp();
    const b = document.createElement("div");
    b.className = "kr-boot" + (web ? " web" : ""); b.setAttribute("aria-hidden", "true");
    b.innerHTML = `<div class="bk"><span class="bmk">${window.krIso ? window.krIso(84, "stack") : ""}</span>` +
      `<span class="bweb">${window.krIso ? window.krIso(40, "breathe") : ""}<b>Krate</b><small>Studio</small></span>` +
      `<span class="hair"><i></i></span><span class="cbar"><i></i></span>` +
      `<p class="bt"><span class="kr-swp"><span>${web ? "Loading Studio" : "Opening Studio"}</span></span></p></div>`;
    document.body.appendChild(b);
    void b.offsetWidth; b.classList.add("run");
    if (window.krStack) window.krStack(b, 120);
    const t0 = performance.now();
    const why = web ? setTimeout(() => window.krSwap && window.krSwap(q(".bt .kr-swp", b), "Waking up the workshop<small>The first visit takes a few seconds.</small>"), 3000) : 0;
    const ready = () => ["viewHome", "viewSession", "viewGate", "viewApps", "viewCloud", "viewIde"].some((id) => { const v = $(id); return v && !v.classList.contains("hidden"); });
    const t = setInterval(() => {
      const el = performance.now() - t0;
      if ((ready() && el > (web ? 700 : 1300)) || el > 20000) {
        clearInterval(t); clearTimeout(why);
        b.classList.add("gone"); setTimeout(() => b.remove(), 700);
      }
    }, 100);
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
      kb.innerHTML = mode === "ide" ? `<kbd>${mod}I</kbd> asks your AI` : mode === "port" ? "Electron · Tauri · web" : "<kbd>↵</kbd> to make";
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
    // A note under the box belongs to the door it was written for; a port
    // or IDE message must not stay under Create (or the other way round).
    const hh = $("homeHint"); if (hh && hh.dataset.mode && hh.dataset.mode !== mode) { hh.textContent = ""; }
    if (hh) hh.dataset.mode = mode;
    if (mode === "create") { const ta = $("homePrompt"); if (ta) ta.focus({ preventScroll: true }); }
  }
  if (tabs) {
    tabs.addEventListener("click", (e) => { const b = e.target.closest("button[data-mode]"); if (b && !b.classList.contains("on")) setMode(b.dataset.mode); });
    (document.fonts && document.fonts.ready ? document.fonts.ready : Promise.resolve()).then(placeInd);
    addEventListener("resize", placeInd);
    setMode("create");
    // New app means the Create box, whichever door was open last.
    const newRow = q('#side .side-row[data-side="home"]');
    // Only a person's own click: the Port and IDE rows press it themselves
    // on the way to their own door.
    if (newRow) newRow.addEventListener("click", (e) => { if (e.isTrusted) setTimeout(() => setMode("create"), 0); });
  }
  ["homePortGo", "homePortDrop", "homePortChoose"].forEach((id) => { const el = $(id); if (el) el.addEventListener("click", startPort); });

  /* ---- Home: the shelf's head is "Your apps (n)" and a chevron ---------- */
  (function shelfHead() {
    const grip = $("shelfGrip"), shelf = $("shelf");
    if (!grip || !shelf) return;
    const tog = document.createElement("button");
    tog.type = "button"; tog.className = "kr-shtog";
    tog.innerHTML = `<span>Your apps</span><em class="kr-shct"></em>${ico("chev", "kr-ico kr-shchev")}`;
    grip.insertAdjacentElement("afterend", tog);
    const sync = () => tog.setAttribute("aria-expanded", shelf.classList.contains("shut") ? "false" : "true");
    tog.addEventListener("click", (e) => {
      // The shelf's head toggles it too; without this one click opened the
      // shelf and the same click, bubbling, shut it again.
      e.stopPropagation();
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
    // Sending: the arrow gives way to a tiny breathing mark until the words
    // are in the conversation.
    if (window.krIso) {
      const sd = document.createElement("span"); sd.className = "kr-sd"; sd.innerHTML = window.krIso(16, "breathe", true);
      send.appendChild(sd);
      send.addEventListener("click", () => {
        if (!field.value.trim() || send.classList.contains("kr-voice") || send.classList.contains("stopping")) return;
        send.classList.add("kr-sending");
        clearTimeout(send._sdT); send._sdT = setTimeout(() => send.classList.remove("kr-sending"), 900);
      }, true);
    }
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
        "sep",
        (app() && app().account && app().account.signed_in !== false && (app().account.login || app().account.name))
          ? { icon: "lock", label: "Sign out", danger: true, run: () => { try { signOutToGate(); } catch (e) {} } }
          : { icon: "user", label: "Sign in", run: () => press($("profSignIn")) },
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
      // The newest version, or "draft" before anything is built -- also for
      // a reopened session, whose transcript has no build rows.
      const paint = () => {
        const chips = qa("#thread .vchip b");
        const last = chips[chips.length - 1];
        const st = app(), sess = st && st.session;
        const word = last ? last.textContent : sess && sess.result ? `v${sess.builds || 1}` : sess ? "draft" : "";
        ver.hidden = !word;
        ver.classList.toggle("draft", word === "draft");
        if (ver.textContent !== word) ver.textContent = word;
      };
      setInterval(paint, 800);
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
  // Krate's words arrive one after another, each settling out of a soft
  // blur, led by a small dot that takes the next of the five colours with
  // every sentence. Only a message that arrives on its own: a reopened
  // session appends its whole history in one go, and replaying that would
  // be a slow show.
  const C5 = ["var(--accent)", "var(--violet)", "var(--pink)", "var(--orange)", "var(--green)"];
  function wordIn(msg) {
    const body = q(":scope > .body", msg);
    if (!body || reduce || body.dataset.krw) return;
    const text = body.textContent;
    if (!text || text.length > 900) return;
    body.dataset.krw = "1";
    body.textContent = "";
    const words = [];
    text.split(/(\s+)/).forEach((part) => {
      if (!part) return;
      if (/^\s+$/.test(part)) { body.appendChild(document.createTextNode(part)); return; }
      const s = document.createElement("span");
      s.className = "kr-w kr-wait"; s.textContent = part; body.appendChild(s); words.push(s);
    });
    const dot = document.createElement("i"); dot.className = "kr-bk";
    let t = 120, sn = 0;
    words.forEach((w) => {
      t += 26 + Math.min(w.textContent.length, 12) * 4 + (/[.,?!:]$/.test(w.textContent) ? 90 : 0);
      const col = C5[sn % 5];
      if (/[.?!:]$/.test(w.textContent)) sn++;
      setTimeout(() => { if (!w.isConnected) return; w.classList.remove("kr-wait"); w.classList.add("kr-on"); w.after(dot); dot.style.setProperty("--c", col); }, t);
    });
    setTimeout(() => { dot.style.opacity = "0"; setTimeout(() => dot.remove(), 450); }, t + 500);
  }
  // The mark beside each of Krate's turns, breathing while that turn is
  // working: planning, or this session's build. Read from app.js's state.
  const working = () => {
    const st = app(); if (!st) return false;
    let plan = false; try { plan = planning; } catch (e) {}
    const bs = st.buildingSession;
    return !!plan || !!(bs && st.session && bs.id === st.session.id && !st.buildSettled);
  };
  function paintAvatars() {
    if (!thread) return;
    let afterYou = true, last = null;
    for (const el of thread.children) {
      if (el.classList.contains("you")) { afterYou = true; continue; }
      if (afterYou && (el.classList.contains("krate") || el.classList.contains("thought") || el.classList.contains("msg"))) {
        afterYou = false;
        let av = q(":scope > .kr-tav", el);
        if (!av) { av = document.createElement("span"); av.className = "kr-tav"; av.setAttribute("aria-hidden", "true"); av.innerHTML = window.krIso ? window.krIso(16) : ""; el.prepend(av); }
        last = av;
      }
    }
    // Krate explains the test windows once; said again on every change it
    // is noise between the steps.
    let seen = false;
    qa(".msg.krate > .body", thread).forEach((b) => {
      if (!/^While I work, I'll open your app/.test(b.textContent)) return;
      b.parentElement.classList.toggle("kr-dup", seen); seen = true;
    });
    const on = working();
    qa(".kr-tav.kwork", thread).forEach((a) => { if (a !== last || !on) a.classList.remove("kwork"); });
    if (last && on && !last.classList.contains("kwork")) last.classList.add("kwork");
    // The first beat: three dots in the live thinking line, for a moment.
    qa(".thought.live > .thought-head", thread).forEach((h) => {
      if (h.dataset.krd) return;
      h.dataset.krd = "1";
      const d = document.createElement("span"); d.className = "kr-kdots kr-kd-in"; d.innerHTML = "<i></i><i></i><i></i>";
      h.prepend(d); setTimeout(() => d.remove(), 1100);
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
    const acts = cardButtons(msg, card, (b, a) => {
      // Build this means build. In Plan mode the press used to be refused
      // ("You are in Plan mode, so nothing was built") right under a card
      // saying "Building this plan"; pressing it is choosing to build, so
      // the mode follows the press, as the design's plan-then-build does.
      if (!port && b.classList.contains("btn-primary")) { try { if (composerMode() === "plan" && window.setWebMode) window.setWebMode("build"); } catch (e) {} }
      settle(a, port ? "Porting this plan" : "Building this plan");
    });
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
  // A question is answered in its own card, the way the design has it.
  // The AI's questions often carry their own examples ("For example: a
  // game, a to-do list, a timer..."); those become chips that fill the
  // answer. Send puts the answer through the composer (app.js's own path,
  // continuePlanning), so it is recorded and builds exactly as a typed
  // answer does. Skipping is a quiet link, not a button beside the answer:
  // it was pressed by people who meant to answer.
  function chipsOf(q) {
    const m = q.match(/(?:for example|e\.g\.|such as|like)[:,]?\s+([^?.]+)[?.]?/i);
    if (!m) return [];
    return m[1].split(/,\s*(?:or\s+)?|\s+or\s+/i).map((x) => x.trim().replace(/^(a|an|the)\s+/i, "")).filter((x) => x && x.length <= 40).slice(0, 6);
  }
  function questionCard(msg, body) {
    const qs = body.split("\n").map((l) => l.replace(/^\s*\d+\.\s*/, "").trim()).filter(Boolean);
    const open = !!q(":scope > .msg-actions", msg);
    const card = document.createElement("div");
    card.className = "kr-qc" + (qs.length === 1 ? " kr-q1" : "");
    card.innerHTML = `<span class="kr-qtag">${ico("msg", "kr-ico")}${qs.length > 1 ? `${qs.length} questions before I build` : "One question before I build"}</span>` +
      qs.map((t, i) => {
        const chips = open ? chipsOf(t) : [];
        return `<div class="kr-qrow" style="animation-delay:${reduce ? 0 : 80 + i * 120}ms"><b class="kr-qq">${qs.length > 1 ? `<i>${i + 1}</i>` : ""}<span>${esc(t)}</span></b>` +
          (open ? (chips.length ? `<div class="kr-qchips">${chips.map((c) => `<button type="button" class="kr-qchip">${esc(c)}</button>`).join("")}</div>` : "") +
          `<input class="kr-qin" type="text" placeholder="${chips.length ? "Or say it your way" : "Your answer"}" aria-label="Answer to question ${i + 1}" autocomplete="off" spellcheck="false">` : "") + `</div>`;
      }).join("");
    if (!open) { msg.classList.add("kr-carded", "kr-q-msg"); msg.appendChild(card); return; }
    const acts = cardButtons(msg, card, (b, a) => settle(a, "Building without an answer"));
    const send = document.createElement("button");
    send.type = "button"; send.className = "btn kr-dark"; send.textContent = qs.length > 1 ? "Send my answers" : "Send my answer";
    acts.prepend(send);
    qa("button", acts).forEach((b) => { if (b._real) { b.className = "kr-qskip"; b.textContent = "Skip, let Krate decide"; b.title = "Krate picks for you; you can change it after"; } });
    const ins = qa(".kr-qin", card);
    qa(".kr-qrow", card).forEach((row) => {
      const input = q(".kr-qin", row);
      row.addEventListener("click", (e) => {
        const c = e.target.closest(".kr-qchip"); if (!c) return;
        if (/something else|anything else|other/i.test(c.textContent)) { qa(".kr-qchip", row).forEach((x) => x.classList.remove("on")); input.value = ""; input.focus(); return; }
        c.classList.toggle("on");
        input.value = qa(".kr-qchip.on", row).map((x) => x.textContent).join(", ");
      });
      input.addEventListener("input", () => qa(".kr-qchip.on", row).forEach((x) => { if (!input.value.includes(x.textContent)) x.classList.remove("on"); }));
    });
    const submit = () => {
      const answers = ins.map((x) => x.value.trim());
      if (!answers.some(Boolean)) { ins[0].focus(); ins[0].classList.add("kr-need"); setTimeout(() => ins[0].classList.remove("kr-need"), 900); return; }
      const text = qs.length > 1 ? answers.map((x, i) => x ? `${i + 1}. ${x}` : "").filter(Boolean).join("\n") : answers[0];
      const box = $("prompt"), go = $("send");
      if (!box || !go) return;
      box.disabled = false; box.value = text; box.dispatchEvent(new Event("input"));
      press(go);
      card.classList.add("kr-answered");
      qa(".kr-qrow", card).forEach((row, i) => { const v = answers[i]; const done = document.createElement("p"); done.className = "kr-qa"; done.textContent = v || "No preference"; qa(".kr-qchips, .kr-qin", row).forEach((x) => x.remove()); row.appendChild(done); });
      acts.innerHTML = `<span class="kr-tr done kr-went"><span class="kr-tic"><span class="kr-ck">${ico("check", "")}</span></span>Answered</span>`;
    };
    send.addEventListener("click", submit);
    ins.forEach((x, i) => x.addEventListener("keydown", (e) => { if (e.key === "Enter") { e.preventDefault(); if (i < ins.length - 1 && !ins[i + 1].value) ins[i + 1].focus(); else submit(); } }));
    msg.classList.add("kr-carded", "kr-q-msg");
    msg.appendChild(card);
    setTimeout(() => { if (ins[0] && document.activeElement === $("prompt")) ins[0].focus({ preventScroll: true }); }, 350);
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
    paintAvatars();
    if (added.length > 2) return;
    added.forEach((n) => {
      n.classList.add("kr-in");
      if (n.matches(".msg.krate") && !n.matches(".vlive, .vok, .vbad, .kr-carded")) wordIn(n);
    });
  });

  // The build's steps as rows under its receipt, from what really happens:
  // the engine's own lines (authoring, "opening your app to see it",
  // building, packing, the permission wall) and every file the AI writes or
  // edits. A breathing mark on the step being done, a tick on each one
  // finished. "Writing src/lib.rs" carries the real file's line count, and
  // a change carries +added -removed against the code it started from (the
  // card being made reads the file; see makingCard).
  const buildText = { key: "", first: null, last: null };
  const lineCount = (t) => (t ? t.replace(/\n+$/, "").split("\n").length : 0);
  function diffOf(a, b) {
    if (a == null || b == null) return null;
    const count = (t) => { const m = new Map(); t.split("\n").forEach((l) => m.set(l, (m.get(l) || 0) + 1)); return m; };
    const A = count(a), B = count(b); let add = 0, del = 0;
    B.forEach((n, l) => { add += Math.max(0, n - (A.get(l) || 0)); });
    A.forEach((n, l) => { del += Math.max(0, n - (B.get(l) || 0)); });
    return { add, del };
  }
  const runs = new Map(); // build key -> { rows: [{id,label,code,em,st}], revise, checks }
  function liveKey() { const st = app(); return st && st.buildingSession ? `${st.buildingSession.id}:${st.startedAt}` : ""; }
  function runFor(key) {
    if (!key) return null;
    let r = runs.get(key);
    if (!r) {
      const st = app();
      r = { rows: [], revise: !!(st && st.buildVersion > 1), checks: 0, file: "" };
      runs.set(key, r);
      add(r, "read", r.revise ? "Reading your app" : "Reading Krate's API");
    }
    return r;
  }
  function add(r, id, label, code) {
    let row = r.rows.find((x) => x.id === id);
    if (row) { if (row.st !== "now" && row.st !== "done") row.st = "now"; return row; }
    r.rows.forEach((x) => { if (x.st === "now") x.st = "done"; });
    row = { id, label, code: code || "", em: "", st: "now" };
    r.rows.push(row);
    return row;
  }
  const SKIP_FILE = /(bindings\.rs|Cargo\.lock|\.agent-|KRATE_AUTHORING|^\/tmp)/;
  function onLine(line) {
    const r = runFor(liveKey()); if (!r) return;
    const clean = String(line).replace(/^=+>\s*/, "").trim();
    if (/^opening your app to see|^running your app to test|^looking at how your app/i.test(clean)) {
      r.checks += /^looking at/i.test(clean) ? 0 : 1;
      const row = add(r, "look", "Opening it to look"); row.em = r.checks ? `${r.checks} check${r.checks === 1 ? "" : "s"}` : "";
    } else if (/^==> building the component|^\s*Compiling /.test(line)) add(r, "build", "Building the component");
    else if (/^==> packing /.test(line)) { const f = (line.match(/([^\\/]+\.krate)\s*$/) || [])[1] || ""; r.file = f; add(r, "pack", "Packing", f || "the file"); }
    else if (/^==> verifying the permission wall/.test(line)) add(r, "wall", "Checking the permission wall");
    else if (/^==> changing the app in its own source/.test(line)) add(r, "read", "Reading your app");
  }
  function onTool(tool, file, path) {
    const r = runFor(liveKey()); if (!r) return;
    const f = file || (path || "").split(/[\\/]/).pop() || "";
    if (!/^(Write|Edit|MultiEdit)$/.test(tool) || !f || SKIP_FILE.test(path || f)) return;
    const rel = /lib\.rs$/.test(f) ? "src/lib.rs" : f;
    add(r, "w:" + rel, /^Edit|^MultiEdit/.test(tool) && rel !== "src/lib.rs" ? "Editing" : "Writing", rel);
  }
  try {
    if (typeof onEngineLine === "function") { const ol = onEngineLine; window.onEngineLine = function (line) { try { onLine(line); } catch (e) {} return ol.apply(this, arguments); }; }
    if (typeof addWorkTool === "function") { const aw = addWorkTool; window.addWorkTool = function (t, f, p) { try { onTool(t, f, p); } catch (e) {} return aw.apply(this, arguments); }; }
  } catch (e) {}
  let toolsFor = null, toolsKey = "";
  function toolRow(row, st) {
    st = st || row.st;
    const tic = st === "done" ? `<span class="kr-ck">${ico("check", "")}</span>` : st === "bad" ? '<span class="kr-bad">!</span>' : st === "stop" ? '<span class="kr-stp"></span>' : (window.krIso ? window.krIso(13, "breathe", true) : '<span class="kr-spin"></span>');
    const em = row.em ? `<em>${row.em}</em>` : "";
    return `<div class="kr-tr${st === "done" ? " done" : ""}" data-st="${st}" data-key="${esc(st + "|" + row.label + "|" + row.code + "|" + row.em)}"><span class="kr-tic">${tic}</span><span>${esc(row.label)}</span>${row.code ? `<code>${esc(row.code)}</code>` : ""}${em}</div>`;
  }
  // The line counts for the file being written, from the card's reading of it.
  function stampWriting(r) {
    const w = r.rows.find((x) => x.id === "w:src/lib.rs");
    if (!w || buildText.key !== liveKey() || buildText.last == null) return;
    if (r.revise) { const d = diffOf(buildText.first, buildText.last); if (d && (d.add || d.del)) w.em = `<span class="kr-add">+${d.add}</span> <span class="kr-del">−${d.del}</span>`; }
    else w.em = `${lineCount(buildText.last)} lines`;
  }
  function paintRows(box, rows, final) {
    const html = rows.map((x) => toolRow(x, final ? final(x, rows) : null));
    const have = qa(".kr-tr", box);
    html.forEach((h, i) => {
      const t = document.createElement("template"); t.innerHTML = h;
      // Compared by state and words, not markup: a row replaced every tick
      // restarts its breathing.
      if (have[i] && have[i].dataset.key === t.content.firstChild.dataset.key) return;
      if (have[i]) have[i].replaceWith(t.content.firstChild); else box.appendChild(t.content.firstChild);
    });
    qa(".kr-tr", box).slice(html.length).forEach((x) => x.remove());
  }
  function paintTools() {
    if (!thread) return;
    const live = q(".msg.vlive", thread);
    if (live) {
      const key = liveKey(); const r = runFor(key); if (!r) return;
      // The stage machinery is the fallback for a step with no line of its own.
      const st = app(), idx = st ? st.stageIndex : -1, keys = stages().map((x) => x.key);
      if (keys[idx] === "write" && !r.rows.some((x) => x.id.startsWith("w:"))) add(r, "w:src/lib.rs", "Writing", "src/lib.rs");
      if (keys[idx] === "test" && !r.rows.some((x) => /build|look|pack|wall/.test(x.id))) add(r, "build", "Building the component");
      stampWriting(r);
      toolsFor = live; toolsKey = key;
      let box = q(":scope > .kr-tools", live);
      if (!box) { box = document.createElement("div"); box.className = "kr-tools"; const chip = q(".vchip", live); (chip || live).insertAdjacentElement("afterend", box); }
      paintRows(box, r.rows);
      // The receipt says what is happening now, in the same words.
      const now = r.rows[r.rows.length - 1];
      const vm = q(".vchip .vm", live);
      if (vm && now) { const words = [now.label, now.code].filter(Boolean).join(" ") + (now.em ? " · " + now.em.replace(/<[^>]+>/g, "") : ""); if (vm.textContent !== words) vm.textContent = words; }
      return;
    }
    // Settled: the chip was rewritten. Put the rows back, finished.
    if (toolsFor && toolsFor.isConnected && !q(":scope > .kr-tools", toolsFor) && toolsKey) {
      const r = runs.get(toolsKey); if (!r) { toolsFor = null; return; }
      const ok = toolsFor.classList.contains("vok");
      const stopped = /stopped/.test(toolsFor.textContent);
      if (ok) { const look = r.rows.find((x) => x.id === "look"); if (look) look.em = r.checks > 1 ? "all passed" : "passed"; }
      const box = document.createElement("div"); box.className = "kr-tools settled";
      paintRows(box, r.rows, (x, rows) => ok || x !== rows[rows.length - 1] ? "done" : stopped ? "stop" : "bad");
      const chip = q(".vchip", toolsFor); (chip || toolsFor).insertAdjacentElement("afterend", box);
      if (ok) settledOk(toolsFor, r);
      toolsFor = null; toolsKey = "";
    }
  }
  // Built: the change's size on the receipt, and one closing line, as the
  // design ends a build. The words are true of what just happened.
  function settledOk(msg, r) {
    const chip = q(".vchip", msg); const st = app(); const res = st && st.session && st.session.result;
    const d = r.revise ? diffOf(buildText.first, buildText.last) : null;
    if (chip && d && (d.add || d.del) && !q(".kr-diff", chip)) {
      const vm = q(".vm", chip); const sp = document.createElement("span"); sp.className = "kr-diff";
      sp.innerHTML = `<span class="kr-add">+${d.add}</span> <span class="kr-del">−${d.del}</span>`;
      (vm || chip.lastChild).before(sp);
    }
    if (!res || res.verdict === "off-request") return;
    const name = appName(), size = res.size || "";
    const words = r.revise
      ? "Done. The change is in, and I opened it again to check it still works."
      : `It is ready. ${name} is one ${size ? size + " " : ""}file that opens on macOS, Windows and Linux. Try it with Run it, or send it to someone.`;
    const el = document.createElement("div"); el.className = "msg krate kr-close";
    el.innerHTML = '<span class="body"></span>'; q(".body", el).textContent = words;
    const tools = q(":scope > .kr-tools", msg);
    (tools || msg).after(el);
    wordIn(el);
  }
  setInterval(() => { paintTools(); syncCards(); paintAvatars(); }, 400);
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
    // Remix beside Open it: the same start-from-this the gallery cards offer.
    const copyBtn = $("detailCopy");
    if (copyBtn) {
      const rx = document.createElement("button");
      rx.type = "button"; rx.className = "btn kr-remix"; rx.innerHTML = ico("spark") + "<span>Remix</span>";
      rx.title = "Start your own app from this one";
      rx.addEventListener("click", () => {
        const desc = (($("detailDesc") || {}).textContent || "").trim();
        const name = ((nm || {}).textContent || "this app").trim();
        remix(desc || name, name);
      });
      copyBtn.before(rx);
    }
  })();

  /* ---- Your apps: find one by name ---------------------------------------- */
  (function findApp() {
    const head = q("#viewApps .head-actions"), grid = $("appsGrid");
    if (!head || !grid) return;
    const box = document.createElement("label");
    box.className = "kr-find";
    box.innerHTML = `${ico("search")}<input type="search" placeholder="Find an app" aria-label="Find an app" autocomplete="off" spellcheck="false">`;
    head.insertBefore(box, head.firstChild);
    const input = q("input", box);
    let empty = null;
    const apply = () => {
      const s = input.value.trim().toLowerCase();
      let shown = 0;
      qa(".app-card", grid).forEach((c) => { const hit = !s || c.textContent.toLowerCase().includes(s); c.hidden = !hit; if (hit) shown++; });
      const nc = q(".kr-newcard", grid); if (nc) nc.hidden = !!s;
      if (s && !shown) {
        if (!empty) { empty = document.createElement("p"); empty.className = "kr-findnone"; grid.appendChild(empty); }
        empty.textContent = `No app called “${input.value.trim()}”. Try another word, or make it.`;
        empty.hidden = false;
      } else if (empty) empty.hidden = true;
    };
    input.addEventListener("input", apply);
    input.addEventListener("keydown", (e) => { if (e.key === "Escape" && input.value) { e.stopPropagation(); input.value = ""; apply(); } });
    watch(grid, { childList: true }, () => { if (input.value) apply(); });
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
      q(".kr-lb", b).innerHTML = `${window.krIso ? window.krIso(15, "breathe", true) : ""}Uploading ${esc((shareApp && shareApp.size) || "the app")}`;
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
  // Publish, in the sheet: the name, one line, and whether it is listed.
  // The fields are app.js's own publish sheet's (it prefills them, and it
  // does the publishing and any sign-in); this only shows them here.
  function paintPub() {
    const pane = q('.kr-wp[data-w="pub"]', shareWrap);
    if (!isCurrent()) { pane.innerHTML = `<p class="kr-shp">Publishing puts the newest version in the gallery. Open it and publish from there.</p>`; return; }
    const sheet = $("publishSheet");
    try { openPublishSheet(); } catch (e) {}
    if (sheet) sheet.classList.add("hidden");
    const nameV = (($("pubName") || {}).value || ""), descV = (($("pubDesc") || {}).value || "");
    pane.innerHTML = `<div class="kr-frm">
        <label>Name<input class="kr-field" data-n maxlength="60"></label>
        <label>One line about it<input class="kr-field" data-d maxlength="140" placeholder="What it does, in a sentence"></label>
        <div class="kr-tog"><span>List it in the gallery<small>Off: only people with the link can find it.</small></span><button type="button" class="sw on" role="switch" aria-checked="true" aria-label="List it in the gallery" data-l></button></div>
        <div class="kr-shacts"><button type="button" class="btn kr-dark" data-pub>${ico("globe")}<span>Publish</span></button><button type="button" class="btn kr-ghost" data-more>Add a picture or a logo</button></div>
        <p class="kr-sherr" hidden></p></div>`;
    const n = q("[data-n]", pane), d = q("[data-d]", pane), l = q("[data-l]", pane), go = q("[data-pub]", pane), err = q(".kr-sherr", pane);
    n.value = nameV; d.value = descV;
    l.addEventListener("click", () => { const on = !l.classList.contains("on"); l.classList.toggle("on", on); l.setAttribute("aria-checked", String(on)); });
    const handOver = () => {
      const pn = $("pubName"), pd = $("pubDesc"), pl = $("pubListed");
      if (pn) pn.value = n.value; if (pd) pd.value = d.value; if (pl) pl.checked = l.classList.contains("on");
    };
    q("[data-more]", pane).addEventListener("click", () => { handOver(); closeShare(); if (sheet) sheet.classList.remove("hidden"); });
    go.addEventListener("click", async () => {
      if (go.dataset.busy) return;
      if (!n.value.trim()) { n.focus(); return; }
      go.dataset.busy = "1"; err.hidden = true;
      handOver();
      const listed = l.classList.contains("on");
      // A ring fills around the mark while it uploads; done, it comes down
      // as the home page's stamp.
      const frm = q(".kr-frm", pane); frm.hidden = true;
      const ring = document.createElement("div"); ring.className = "kr-pubing";
      ring.innerHTML = `<div class="kr-pring"><svg class="rg" viewBox="0 0 210 210"><defs><path id="krPubR" d="M105 105 m-80 0 a80 80 0 1 1 160 0 a80 80 0 1 1 -160 0"/></defs><circle class="trk2" cx="105" cy="105" r="98"/><circle class="prog" cx="105" cy="105" r="98" pathLength="1"/><text class="rtx" font-size="14" font-weight="600"><textPath href="#krPubR" textLength="495" lengthAdjust="spacing">PUBLISHING · TO THE GALLERY · PUBLISHING · TO THE GALLERY ·</textPath></text></svg>${window.krIso ? window.krIso(46, "breathe") : ""}` +
        `<svg class="st" viewBox="0 0 210 210"><defs><path id="krPubS" d="M105 105 m-78 0 a78 78 0 1 1 156 0 a78 78 0 1 1 -156 0"/></defs><g fill="none" stroke="currentColor" stroke-width="5"><circle cx="105" cy="105" r="98"/><circle cx="105" cy="105" r="62"/></g><text font-size="15" font-weight="700" fill="currentColor"><textPath href="#krPubS" textLength="482" lengthAdjust="spacing">${listed ? "IN THE GALLERY · ANYONE CAN OPEN IT · " : "PUBLISHED · ONLY WITH THE LINK · "}</textPath></text><text x="105" y="119" text-anchor="middle" font-size="40" font-weight="700" fill="currentColor">LIVE</text></svg></div>` +
        `<p class="pc"><span class="kr-swp"><span>Getting it ready</span></span></p>`;
      frm.after(ring);
      const pr = q(".prog", ring), t0 = performance.now();
      let upT = setTimeout(() => window.krSwap(q(".kr-swp", ring), `Uploading ${esc((shareApp && shareApp.size) || "it")}`), 900);
      let raf = 0; const fill = (now) => { const p = Math.min(.86, (now - t0) / 3200), e = 1 - Math.pow(1 - p, 2.2); pr.style.strokeDashoffset = 1 - e; raf = requestAnimationFrame(fill); }; raf = requestAnimationFrame(fill);
      linking = true;
      try { await publishFromSheet(); } catch (e) {}
      linking = false;
      cancelAnimationFrame(raf); clearTimeout(upT);
      if (linkOf() && !($("pubSignin") && !$("pubSignin").classList.contains("hidden"))) {
        pr.style.transition = "stroke-dashoffset .4s var(--ease)"; pr.style.strokeDashoffset = 0;
        await new Promise((r) => setTimeout(r, 380));
        q(".kr-pring", ring).classList.add("done"); window.krSwap(q(".kr-swp", ring), listed ? "In the gallery" : "Published");
        await new Promise((r) => setTimeout(r, reduce ? 0 : 1100));
      }
      ring.remove(); frm.hidden = false;
      if (sheet) sheet.classList.add("hidden");
      const url = linkOf();
      const signIn = $("pubSignin") && !$("pubSignin").classList.contains("hidden");
      if (url && !signIn) return published(pane, n.value.trim(), url, listed);
      if (signIn) { closeShare(); if (sheet) sheet.classList.remove("hidden"); return; }
      delete go.dataset.busy; q("span", go).textContent = "Publish";
      err.textContent = (($("pubNote") || {}).textContent || "").trim() || "It did not publish just now. Try again in a moment."; err.hidden = false;
    });
  }
  function published(pane, name, url, listed) {
    const burst = reduce ? "" : `<div class="kr-pburst">${Array.from({ length: 14 }, (_, i) => { const a = i / 14 * Math.PI * 2, d = 60 + (i % 3) * 18;
      return `<i style="--c:${["#3d6df0", "#22c55e", "#e8873f", "#7c5ce8", "#d6578f"][i % 5]};--x:${Math.cos(a) * d}px;--y:${Math.sin(a) * d}px;animation-delay:${i * 8}ms"></i>`; }).join("")}</div>`;
    pane.innerHTML = `<div class="kr-pubdone">${burst}<div class="kr-big">${ico("check", "")}</div><b></b><p>${listed ? "Anyone can find it in the gallery, and the link opens it." : "Not listed: only people with the link can find it."}</p></div>
      <div class="kr-linkf"><span class="kr-u"></span><button type="button" class="btn kr-dark" data-copy>${ico("copy")}<span>Copy</span></button></div>
      ${listed ? `<div class="kr-shacts kr-center"><button type="button" class="btn kr-ghost" data-gal>See it in the gallery</button></div>` : ""}`;
    q("b", pane).textContent = listed ? `${name} is in the gallery` : `${name} has a link`;
    q(".kr-u", pane).textContent = url.replace(/^https?:\/\//, "");
    q("[data-copy]", pane).addEventListener("click", (e) => {
      const b = e.currentTarget, done = () => { b.classList.add("ok"); q("span", b).textContent = "Copied"; setTimeout(() => { b.classList.remove("ok"); q("span", b).textContent = "Copy"; }, 1400); };
      (navigator.clipboard ? navigator.clipboard.writeText(url) : Promise.reject()).then(done, done);
    });
    const g = q("[data-gal]", pane);
    if (g) g.addEventListener("click", () => { closeShare(); const r = q('#side .side-row[data-side="discover"]'); if (r) r.click(); });
    paintLink(url, false);
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
  const WELCOME_KEY = "krate-welcome-v3";
  const obWrap = document.createElement("div");
  obWrap.className = "kr-ob"; obWrap.id = "krOb";
  obWrap.setAttribute("role", "dialog"); obWrap.setAttribute("aria-modal", "true"); obWrap.setAttribute("aria-labelledby", "krObT");
  // The welcome: the mark stacks in, the headline's word turns through a
  // few apps in the home page's colours, and one small line shows the
  // sentence for each becoming its file. Nothing else moves.
  const OB_APPS = [["habit tracker", "a habit tracker with streaks", "var(--accent)"], ["bill splitter", "split a dinner bill, tip included", "var(--green)"], ["focus timer", "a pomodoro focus timer", "var(--orange)"], ["colour palette", "pick colours from a photo", "var(--violet)"]];
  const fileName = (w) => w.replace(/[^a-z0-9]+/gi, "-").toLowerCase() + ".krate";
  obWrap.innerHTML = `<div class="kr-ob-card kr-ob3" tabindex="-1">
    <button type="button" class="kr-x kr-ob-x" aria-label="Close">${ico("x")}</button>
    <span class="kr-obmk">${window.krIso ? window.krIso(38, "stack") : ""}</span>
    <h2 id="krObT">Say what you want.<br>Get a <span class="kr-obw">${OB_APPS.map((x, i) => `<span class="w${i ? "" : " on"}" style="--c:${x[2]}">${x[0]}</span>`).join("")}</span></h2>
    <p class="kr-ob-s">Krate Studio turns one sentence into a small app that opens on every desktop.</p>
    <div class="kr-oblive" aria-hidden="true"><div class="kr-obcmp"><span class="tx"></span><span class="car"></span></div>
      <div class="kr-obfile"><img src="krate-doc.png" alt=""><b></b><small>opens on every desktop</small></div></div>
    <div class="kr-ob-f"><button type="button" class="kr-plain kr-ob-skip">Skip</button><span class="kr-dots">${OB_APPS.map((x, i) => `<i${i ? "" : ' class="on"'}></i>`).join("")}</span><button type="button" class="kr-dark kr-ob-go">Take the tour</button></div>
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
    const my = ++pvRun, card = q(".kr-ob3", obWrap), live = q(".kr-oblive", obWrap), tx = q(".kr-obcmp .tx", obWrap), ww = q(".kr-obw", obWrap), dots = qa(".kr-dots i", obWrap);
    const on = () => my === pvRun && obWrap.classList.contains("on");
    qa(".kr-obmk .ly", obWrap).forEach((l) => l.classList.remove("on")); if (window.krStack) window.krStack(obWrap, 250);
    const word = (i) => {
      const ws = qa(".w", ww), prev = ws.findIndex((w) => w.classList.contains("on"));
      ws.forEach((w, k) => { w.classList.toggle("on", k === i); w.classList.toggle("out", k === prev && k !== i); });
      ww.style.width = ws[i].offsetWidth + "px";
    };
    for (let i = 0; on(); i = (i + 1) % OB_APPS.length) {
      const [w, prompt, col] = OB_APPS[i];
      live.style.setProperty("--c", col); card.style.setProperty("--c", col); word(i);
      dots.forEach((d, k) => d.classList.toggle("on", k === i));
      live.classList.remove("made"); tx.textContent = "";
      if (reduce) { tx.textContent = prompt; q(".kr-obfile b", obWrap).textContent = fileName(w); live.classList.add("made"); return; }
      await sleep(450);
      for (let k = 1; k <= prompt.length; k++) { if (!on()) return; tx.textContent = prompt.slice(0, k); await sleep(1400 / prompt.length); }
      await sleep(350); if (!on()) return;
      q(".kr-obfile b", obWrap).textContent = fileName(w); live.classList.add("made");
      await sleep(2600);
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
    try { localStorage.setItem(WELCOME_KEY, "1"); } catch (e) {}
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
    { c: "var(--accent)", el: () => firstVisible(q("#viewHome .composer-shell .bigbar"), q("#viewHome .composer-shell")), t: "Say what you want",
      p: () => desktopApp() ? "Describe an app in a sentence, like “a habit tracker with streaks”. Your AI writes it, and Krate builds it, opens it and checks it." : "Describe an app in a sentence, like “a habit tracker with streaks”. Krate’s AI writes it, builds it, opens it and checks it." },
    { c: "var(--violet)", el: () => firstVisible($("builtByChip"), $("agentChip")), t: () => desktopApp() ? "Use the AI you already have" : "Krate’s AI does the writing",
      p: () => desktopApp() ? "Claude, Codex, Gemini and others. Krate works with whichever one you pick, and you can change it here at any time." : "Here it is Krate’s own. In Studio on your computer you can pick Claude, Codex or Gemini instead." },
    { c: "var(--orange)", el: () => firstVisible(q("#side .side-nav")), can: () => !!q("#side .side-nav"), side: "right", before: () => openSide(true), t: "Every app is one small file",
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
    card.style.setProperty("--c", st.c || "var(--accent)"); hole.style.setProperty("--c", st.c || "var(--accent)");
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
  // The welcome is new, so everyone sees it once -- not only a first run.
  // Somebody who onboarded before it existed has never seen it, and the
  // only other way to it was a row in Settings. Shown on Home, never over
  // work in progress, and then never again on this computer.
  (function welcomeOnce() {
    const seen = () => { try { return localStorage.getItem(WELCOME_KEY) === "1"; } catch (e) { return true; } };
    if (seen()) return;
    let tries = 0;
    const t = setInterval(() => {
      // Closed by the first-run path meanwhile: that was this welcome.
      if (seen()) { clearInterval(t); return; }
      const home = $("viewHome");
      const busy = qa(".sheet-wrap:not(.hidden)").length || obWrap.classList.contains("on") || tourEl.classList.contains("on");
      if (home && !home.classList.contains("hidden") && !busy) { clearInterval(t); obOpen(); }
      else if (++tries > 40) clearInterval(t);
    }, 250);
  })();

  /* ---- the Code pane: open the app's source in the IDE ------------------- */
  (function codeToIde() {
    const head = q("#paneCode .code-head"), copy = $("codeCopy");
    if (!head || !copy || !desktopApp()) return;
    const b = document.createElement("button");
    b.type = "button"; b.className = "btn btn-sm kr-toide"; b.innerHTML = ico("code") + "<span>Open in the IDE</span>";
    b.title = "Edit this app's source with a live preview, and ask your AI from there";
    b.addEventListener("click", async () => {
      let a = null; try { a = currentApp(); } catch (e) {}
      let dir = ""; try { dir = await sourceDirOf(a); } catch (e) {}
      if (!dir || !window.krIdeOpen) { toast("This app's source is not on this computer"); return; }
      const name = ((a && a.name) || "").replace(/\.krate$/, "") || undefined;
      window.krIdeOpen(dir, name);
    });
    copy.before(b);
  })();

  /* ---- a build that did not finish: the card it would have been ---------- */
  (function failed() {
    const st = $("stateFailed"), card = q("#stateFailed .fail-card");
    if (!st || !card) return;
    const slab = (y) => `<g><path class="l" d="M28 ${y} L80 ${y + 26} L80 ${y + 41} L28 ${y + 15}Z"/><path class="r" d="M80 ${y + 26} L132 ${y} L132 ${y + 15} L80 ${y + 41}Z"/><path class="t" d="M28 ${y} L80 ${y - 26} L132 ${y} L80 ${y + 26}Z"/></g>`;
    const ghost = document.createElement("div");
    ghost.className = "kr-failghost"; ghost.setAttribute("aria-hidden", "true");
    ghost.innerHTML = `<div class="kr-fg-th"><svg viewBox="14 30 132 126">${slab(104)}${slab(84)}${slab(64)}</svg></div><div class="kr-fg-nm"><b></b><small>Did not finish</small></div>`;
    st.insertBefore(ghost, card);
    const paint = () => {
      if (st.classList.contains("hidden")) return;
      q("b", ghost).textContent = appName();
      // The words follow what stopped it: a build that was stopped is not a failure.
      const t = (($("failTitle") || {}).textContent || "").toLowerCase();
      q("small", ghost).textContent = /stop/.test(t) ? "Stopped" : /open|run|start/.test(t) ? "Did not open" : "Did not finish";
    };
    watch(st, { attributes: true, attributeFilter: ["class"] }, paint);
    watch($("failTitle"), { childList: true, characterData: true, subtree: true }, paint);
    paint();
  })();

  /* ---- What is free: After that, Set up ---------------------------------- */
  const planSetUp = $("planSetUp");
  if (planSetUp) planSetUp.addEventListener("click", () => {
    const ps = $("planSheet"); if (ps) ps.classList.add("hidden");
    try { openAiSheet(); } catch (e) {}
  });

  /* ---- the app's name in the session: its menu ---------------------------- */
  (function titleMenu() {
    const t = $("sessTitleBtn");
    if (!t) return;
    // Renaming happens inside this button, and the Enter that ends it also
    // reached the button as a click, opening the menu again. A click while
    // the name is being edited, or just after, is not a request for the menu.
    let editEnd = 0;
    const nameEl = $("railTitle");
    if (nameEl) nameEl.addEventListener("focusout", () => { editEnd = Date.now(); });
    t.addEventListener("click", (e) => {
      if (bypass) return;
      e.stopImmediatePropagation(); e.preventDefault();
      if ((nameEl && nameEl.isContentEditable) || Date.now() - editEnd < 500) return;
      let a = null; try { a = currentApp(); } catch (err) {}
      const st = app();
      const items = [{ icon: "pencil", label: "Rename", run: () => { try { renameSessionTitle(); } catch (err) {} } }];
      if (a && desktopApp() && window.krIdeOpen) items.push({ icon: "code", label: "Open in the IDE", run: async () => {
        let dir = ""; try { dir = await sourceDirOf(a); } catch (err) {}
        if (dir) window.krIdeOpen(dir, (a.name || "").replace(/\.krate$/, "") || undefined); else toast("This app's source is not on this computer");
      } });
      let label = ""; try { label = agentLabel(); } catch (err) {}
      items.push({ mark: logo(st && st.agent), label: "Change the AI", em: label, run: () => agentPicker(t) });
      if (a) items.push({ icon: desktopApp() ? "folder" : "down", label: desktopApp() ? "Show in folder" : "Download", run: () => press($("filesSave")) });
      items.push("sep", { icon: "trash", label: "Remove this app", danger: true, keep: true, run: (row) => {
        if (!row.dataset.armed) { row.dataset.armed = "1"; q(".kr-pt", row).firstChild.textContent = "Click again to remove it"; return true; }
        try { removeCurrentSession(); } catch (err) {}
      } });
      openPop(t, items);
    }, true);
  })();

  /* ---- the receipt row: the build as a file, with what happened to it ----- */
  // app.js draws the row (vlive, then vok or vbad) and keeps its words and
  // buttons; this adds the design's mark at the start and the running time.
  const clockOf = (n) => `${Math.floor(n / 60)}:${String(n % 60).padStart(2, "0")}`;
  function paintChips() {
    if (!thread) return;
    qa(".msg.vlive > .vchip, .msg.vok > .vchip, .msg.vbad > .vchip", thread).forEach((c) => {
      const kind = c.parentElement.classList.contains("vlive") ? "live" : c.parentElement.classList.contains("vok") ? "ok" : /stopped/.test(c.textContent) ? "stop" : "bad";
      let ri = q(":scope > .kr-ri", c);
      if (!ri || ri.dataset.k !== kind) {
        if (ri) ri.remove();
        ri = document.createElement("span"); ri.className = "kr-ri"; ri.dataset.k = kind;
        ri.innerHTML = kind === "live" ? (window.krIso ? window.krIso(14, "breathe") : "") : kind === "ok" ? `<i class="ok">${ico("check", "")}</i>` : kind === "stop" ? '<i class="no"></i>' : '<i class="bad">!</i>';
        c.prepend(ri);
      }
      if (kind === "live") {
        let re = q(":scope > .kr-re", c);
        if (!re) { re = document.createElement("span"); re.className = "kr-re"; const vm = q(".vm", c); (vm || c.lastChild).after(re); }
        const st = app(); const t0 = st && st.startedAt;
        re.textContent = t0 ? clockOf(Math.max(0, Math.floor((Date.now() - t0) / 1000))) : "";
      }
    });
  }
  setInterval(paintChips, 500);
  watch(thread, { childList: true, subtree: true }, () => requestAnimationFrame(paintChips));

  /* ---- the card being made: the code, as the AI writes it ----------------- */
  // While an app is being made, Studio reads the build's own src/lib.rs and
  // shows it on the card: the starter first, then each change the AI makes,
  // streaming in a few characters at a time. Lines already on the card stay;
  // only what changed is written. Where the file cannot be read (the web
  // Studio, or before the project exists) the crate stays. After 30 s the
  // status is the home page's sentence; after 45 s a note says bigger apps
  // take a minute.
  (function makingCard() {
    const box = $("buildShotBox"), view = $("stateBuilding");
    if (!box || !view) return;
    const card = document.createElement("div");
    card.className = "kr-code"; card.setAttribute("aria-hidden", "true");
    card.innerHTML = '<div class="kr-cw"><div class="kr-wc"><div class="kr-hlb"></div></div></div>';
    box.appendChild(card);
    const wc = q(".kr-wc", card), hlb = q(".kr-hlb", card);
    const title = $("buildTitle"), peek = $("peekBox");
    const snt = document.createElement("p"); snt.className = "kr-snt"; snt.hidden = true;
    if (peek) peek.after(snt);
    const still = document.createElement("div"); still.className = "kr-still";
    still.innerHTML = `<div><p>${window.krIso ? window.krIso(16, "breathe", true) : ""}Still going. Bigger apps take a minute.</p></div>`;
    (q(".build-elapsed", view) || view).before(still);
    const KW = /\b(use|struct|enum|fn|let|mut|impl|pub|for|in|if|else|match|const|static|self|Self|return|true|false|Some|None|Ok|Err|mod|crate|extern|as|where|while|loop|break|continue|move|ref|type|trait)\b/;
    const TOK = /(\/\/.*$)|("(?:[^"\\]|\\.)*")|\b(use|struct|enum|fn|let|mut|impl|pub|for|in|if|else|match|const|static|self|Self|return|true|false|Some|None|Ok|Err|mod|crate|extern|as|where|while|loop|break|continue|move|ref|type|trait)\b|\b(u8|u16|u32|u64|usize|i32|i64|f32|f64|bool|str|char|[A-Z][A-Za-z0-9_]*)\b|\b([a-z_][a-z0-9_]*)(?=\(|!)|\b(\d+(?:\.\d+)?)\b|(\s+|[A-Za-z_][A-Za-z0-9_]*|::|->|=>|[^\sA-Za-z0-9_])/g;
    const NAMES = ["tk-c", "tk-s", "tk-k", "tk-t", "tk-f", "tk-n", ""];
    const tokens = (l) => { const out = []; let m; TOK.lastIndex = 0; while ((m = TOK.exec(l))) { const g = m.slice(1).findIndex((x) => x !== undefined); out.push([m[0], NAMES[g]]); } return out; };
    let lines = [], rows = [], queue = [], sessionId = null, dir = "", busy = false, lastText = null, lastAt = 0, startAt = 0;
    const LH = 18;
    function reset() { lines = []; rows = []; queue = []; wc.querySelectorAll(".kr-wl").forEach((r) => r.remove()); lastText = null; dir = ""; card.classList.remove("on", "built"); wc.style.transform = ""; }
    function row(i) {
      const r = document.createElement("div"); r.className = "kr-wl kr-wait";
      r.innerHTML = `<span class="kr-wn">${i + 1}</span><span class="kr-wt"></span>`; return r;
    }
    function setText(text) {
      const next = text.replace(/\r/g, "").split("\n");
      let same = 0; while (same < lines.length && same < next.length && lines[same] === next[same]) same++;
      // Lines past the first difference are rewritten.
      // Lines that stay but were still waiting to appear keep their place in
      // the queue; dropping them left gaps the highlight then drifted past.
      const kept = new Set(rows.slice(0, same));
      rows.slice(same).forEach((r) => r.remove()); rows = rows.slice(0, same);
      queue = queue.filter((it) => kept.has(it.r));
      for (let i = same; i < next.length; i++) {
        const r = row(i); wc.appendChild(r); rows.push(r);
        const tx = q(".kr-wt", r);
        if (!next[i].trim()) { queue.push({ r, el: null }); continue; }
        tokens(next[i]).forEach(([t, cls]) => {
          const el = document.createElement("span"); el.className = "kr-wk" + (cls ? " " + cls : ""); el.textContent = t; tx.appendChild(el);
          if (t.trim()) queue.push({ r, el }); else el.classList.add("shown");
        });
      }
      lines = next;
      // Line numbers follow the rows that stayed.
      rows.forEach((r, i) => { q(".kr-wn", r).textContent = i + 1; });
    }
    let car = document.createElement("i"); car.className = "kr-car";
    function step() {
      if (!queue.length) return;
      // Faster when there is a lot still to write, so a whole file never
      // takes longer than a few seconds to appear.
      const n = Math.max(1, Math.ceil(queue.length / 90));
      for (let k = 0; k < n && queue.length; k++) {
        const it = queue.shift();
        it.r.classList.remove("kr-wait");
        if (it.el) { it.el.classList.add("shown"); it.el.after(car); }
        const i = rows.indexOf(it.r);
        hlb.style.transform = `translateY(${i * LH}px)`;
        const H = card.clientHeight, V = Math.floor((H - 12) / LH), a = Math.floor(V * 0.5);
        wc.style.transform = `translateY(${-Math.max(0, i - a) * LH}px)`;
      }
    }
    setInterval(() => { if (card.classList.contains("on")) step(); }, 38);
    const PHW = { read: ["planning", "var(--ink-2)"], write: ["writing", "var(--accent)"], test: ["building", "var(--accent)"], done: ["packing", "var(--orange)"] };
    async function tick() {
      const st = app();
      const live = st && st.buildingSession && !view.classList.contains("hidden");
      if (!live) { if (sessionId) { sessionId = null; reset(); } snt.hidden = true; still.classList.remove("on"); return; }
      if (sessionId !== st.buildingSession.id || startAt !== st.startedAt) { sessionId = st.buildingSession.id; startAt = st.startedAt; reset(); }
      if (title) { const nm = appName(); if (nm && title.textContent !== nm) title.textContent = nm; }
      const secs = st.startedAt ? (Date.now() - st.startedAt) / 1000 : 0;
      // Past 30 s: the sentence, its word turning with each step.
      const stage = (stages()[st.stageIndex] || {}).key || "read";
      const w = PHW[stage] || PHW.write;
      if (secs >= 30) {
        snt.hidden = false;
        const h = `Krate is <span class="w" style="--c:${w[1]}">${w[0]}</span> your ${esc(appName().toLowerCase())}`;
        if (snt.dataset.k !== stage) { snt.dataset.k = stage; snt.innerHTML = h; }
      } else snt.hidden = true;
      still.classList.toggle("on", secs >= 45);
      if (!desktopApp() || busy || Date.now() - lastAt < 1400) return;
      busy = true; lastAt = Date.now();
      try {
        if (!dir) dir = (await invoke("session_source_dir", { session: sessionId })) || "";
        if (dir) {
          const text = await invoke("ide_read", { path: dir, rel: "src/lib.rs" });
          if (sessionId && typeof text === "string" && text !== lastText) {
            lastText = text; setText(text); card.classList.add("on");
            const k = `${sessionId}:${startAt}`;
            if (buildText.key !== k) { buildText.key = k; buildText.first = text; }
            buildText.last = text;
          }
        }
      } catch (e) { /* not there yet, or not readable: the crate stays */ }
      busy = false;
    }
    setInterval(tick, 500);
  })();

  /* ---- the Gallery, as the design has it ---------------------------------- */
  // Drawn from what app.js already loaded from the hub (state.cloud, the
  // shelves), into the design's layout: the title and search, categories as
  // one pill row, Popular / New / Smallest, the most-opened app featured,
  // and the rest as app windows. Every press is app.js's: a card opens its
  // page (showCloudApp), a category or a search asks the hub (openCloud),
  // Get the file is the hub's own download (?dl=1). Only what the hub says
  // is shown: no prompt it does not store, no count it did not send.
  const galView = $("viewCloud");
  const gal = { sort: "pop", list: [], featuredTyped: "" };
  const kb = (a) => (a.meta && a.meta.size ? Math.max(1, Math.round(a.meta.size / 1024)) + " KB" : "");
  const fileOf = (a) => ((a.meta && a.meta.name) || "app").toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "") + ".krate";
  const netOf = (a) => { const c = a.meta && a.meta.capabilities; return Array.isArray(c) ? c.some((x) => /^net\./.test(String(x))) : null; };
  const chipOf = (a) => `<span class="kr-kchip"><img src="krate-doc.png" alt="">${esc(fileOf(a))}${kb(a) ? `<em>${esc(kb(a))}</em>` : ""}</span>`;
  const reachOf = (a) => { const n = netOf(a); return n === null ? "" : n ? `<span class="kr-kreach net">${ico("globe")}Asks for the network</span>` : `<span class="kr-kreach">${ico("lock")}Offline</span>`; };
  const catName = (id) => { let l = id; try { l = catLabel(id); } catch (e) {} return String(l || "").replace(/^\w/, (c) => c.toUpperCase()); };
  const getUrl = (a) => (a.url || "") + "?dl=1";
  const keyOf = (a) => String(a.id || a.url || (a.meta && a.meta.name) || "");
  function getFile(a, btn) {
    if (!a || !a.url) return;
    if (btn) { btn.classList.add("ing"); setTimeout(() => { btn.classList.remove("ing"); btn.classList.add("got"); const t = q(".gt", btn); if (t) t.textContent = "Downloading"; }, 1300); }
    if (desktopApp()) { try { invoke("open_external", { url: getUrl(a) }); } catch (e) {} }
    else { const l = document.createElement("a"); l.href = getUrl(a); l.download = fileOf(a); document.body.appendChild(l); l.click(); l.remove(); }
    toast(`${fileOf(a)} is downloading`);
  }
  const getBtn = (a, big) => `<button type="button" class="kr-get${big ? " big" : ""}" data-get="${esc(keyOf(a))}"><span class="gt">${big ? "Get the file" : "Get"}</span>${window.krIso ? window.krIso(20, "gfill") : ""}</button>`;
  function win(a, big) {
    const name = esc((a.meta && a.meta.name) || "App");
    return `<div class="kr-kwin${big ? " big" : ""}"><div class="fb"><span class="kr-lights"><i></i><i></i><i></i></span><span>${name}</span></div><div class="kb">${a.shot ? `<img src="${esc(a.shot)}" alt="" loading="lazy">` : `<span class="kb-none">${window.krIso ? window.krIso(40, "", true) : ""}</span>`}</div>${big ? "" : `<div class="khov"><button type="button" class="kr-hbtn" data-remix="${esc(keyOf(a))}">${ico("spark")}Remix</button>${getBtn(a)}</div>`}</div>`;
  }
  function card(a, i) {
    const m = a.meta || {};
    return `<div class="kr-kcard" style="--k:${i}" data-app="${esc(keyOf(a))}" role="button" tabindex="0">${win(a)}
      <div class="kmeta"><b>${esc(m.name || "Untitled app")}</b><small>${m.author ? "@" + esc(m.author) : esc(catName((a.cats && a.cats[0]) || m.category || "apps"))}</small></div>
      ${m.description ? `<p class="ksaid">${esc(m.description)}</p>` : ""}
      <div class="kfoot">${chipOf(a)}${reachOf(a)}</div></div>`;
  }
  function allApps() {
    // What app.js was last asked to draw: the shelves (most opened first,
    // in the hub's order) or a list of results.
    const seen = new Set();
    const src = gal.shown || ((app() && app().cloud) || []);
    return src.filter((a) => { if (!a || !a.meta) return false; const k = keyOf(a); if (seen.has(k)) return false; seen.add(k); return true; });
  }
  function sorted(list) {
    const l = [...list];
    if (gal.sort === "small") l.sort((x, y) => ((x.meta.size || 1e12) - (y.meta.size || 1e12)));
    else if (gal.sort === "new") l.sort((x, y) => (y.meta.published || 0) - (x.meta.published || 0));
    else l.sort((x, y) => (y.opens || 0) - (x.opens || 0) || (y.meta.published || 0) - (x.meta.published || 0));
    return l;
  }
  let galRoot = null;
  function galShell() {
    if (galRoot || !galView) return galRoot;
    const main = q("main.cloud", galView);
    galRoot = document.createElement("div"); galRoot.className = "kr-gal";
    galRoot.innerHTML = `<div class="kr-galh"><div><h1>Gallery</h1><p>Apps people made with Krate and shared. Each one is a single file that opens on every desktop.</p></div>
        <label class="kr-find kr-galq">${ico("search")}<input type="search" placeholder="Search apps" aria-label="Search the gallery" autocomplete="off" spellcheck="false"></label></div>
      <div class="kr-galbar"><div class="kr-cats" role="tablist"></div><div class="kr-sorts" role="tablist"><button type="button" data-s="pop" class="on">Popular</button><button type="button" data-s="new">New</button><button type="button" data-s="small">Smallest</button></div></div>
      <div class="kr-galbody"></div>`;
    main.prepend(galRoot);
    galView.classList.add("kr-galon");
    const qIn = q(".kr-galq input", galRoot), real = $("cloudSearch");
    qIn.addEventListener("input", () => { if (real) { real.value = qIn.value; real.dispatchEvent(new Event("input")); } });
    q(".kr-sorts", galRoot).addEventListener("click", (e) => { const b = e.target.closest("[data-s]"); if (!b) return; gal.sort = b.dataset.s; qa(".kr-sorts button", galRoot).forEach((x) => x.classList.toggle("on", x === b)); paintGal(); });
    q(".kr-cats", galRoot).addEventListener("click", (e) => {
      const b = e.target.closest("[data-c]"); if (!b) return;
      const st = app(); if (st) st.cloudCat = b.dataset.c;
      try { openCloud(); } catch (err) {}
    });
    galRoot.addEventListener("click", (e) => {
      const g = e.target.closest("[data-get]");
      if (g) { e.stopPropagation(); getFile(allApps().find((a) => keyOf(a) === g.dataset.get), g); return; }
      const r = e.target.closest("[data-remix]");
      if (r) { e.stopPropagation(); const a = allApps().find((x) => keyOf(x) === r.dataset.remix); if (a) remix((a.meta.description || a.meta.name || ""), a.meta.name || "this app"); return; }
      const c = e.target.closest("[data-app]");
      if (c) { const a = allApps().find((x) => keyOf(x) === c.dataset.app); if (a) { try { showCloudApp(a); } catch (err) {} } }
    });
    galRoot.addEventListener("keydown", (e) => { if ((e.key === "Enter" || e.key === " ") && e.target.matches("[data-app]")) { e.preventDefault(); e.target.click(); } });
    return galRoot;
  }
  function paintCats() {
    const st = app(); const box = q(".kr-cats", galRoot); if (!st || !box) return;
    let cats = []; try { cats = CLOUD_CATS; } catch (e) {}
    const present = st.cloudCats || null;
    const cur = st.cloudCat || "all";
    box.innerHTML = cats.filter((c) => c.id === "all" || !present || present.includes(c.id))
      .map((c) => `<button type="button" data-c="${c.id}" class="${c.id === cur ? "on" : ""}">${c.id === "all" ? "All" : esc(c.label)}</button>`).join("");
  }
  function skeleton() {
    const body = q(".kr-galbody", galShell()); if (!body) return;
    body.innerHTML = `<div class="kr-kf sk"></div><div class="kr-kgrid">${Array.from({ length: 6 }, (_, i) => `<div class="kr-skc" style="--c:${C5[i % 5]}"><div class="th">${window.krIso ? window.krIso(44, "", true) : ""}</div><span class="l1"></span><span class="l2"></span></div>`).join("")}</div>`;
    const cards = qa(".kr-skc", body), order = [0, 4, 2, 1, 5, 3]; let n = 0;
    clearInterval(gal.lit);
    gal.lit = setInterval(() => { cards.forEach((c) => c.classList.remove("lit")); const c = cards[order[n++ % 6]]; if (c && c.isConnected) c.classList.add("lit"); else clearInterval(gal.lit); }, 280);
    topBar(true);
  }
  async function typeSaid(el, text) {
    gal.featuredTyped = text;
    for (let i = 0; i <= text.length; i++) {
      if (!el.isConnected || gal.featuredTyped !== text) return;
      el.textContent = text.slice(0, i);
      await new Promise((r) => setTimeout(r, reduce ? 0 : i < 2 ? 200 : 26));
    }
  }
  function paintGal() {
    const body = q(".kr-galbody", galShell()); if (!body) return;
    clearInterval(gal.lit); topBar(false);
    paintCats();
    const st = app();
    const query = ((q(".kr-galq input", galRoot) || {}).value || "").trim();
    const list = sorted(allApps());
    const cur = (st && st.cloudCat) || "all";
    const pick = cur === "all" && !query && gal.sort === "pop" ? list.find((a) => a.shot && a.meta.description) : null;
    const rest = list.filter((a) => a !== pick);
    let html = "";
    if (pick) {
      const m = pick.meta;
      // The hub counts opens over 30 days (openCounts in cloud/worker).
      const eye = pick.opens > 0 ? "Most opened this month" : "New in the gallery";
      html += `<div class="kr-kf" data-app="${esc(keyOf(pick))}" role="button" tabindex="0"><div class="kf-l"><span class="kf-eye">${eye}</span><p class="kf-said">“<span class="kf-tx"></span>”<i class="car"></i></p>
        <div class="kf-by">${m.author ? `<span class="kr-av sm">${esc(m.author.charAt(0).toUpperCase())}</span>@${esc(m.author)} made ${esc(m.name || "it")} with Krate.` : `${esc(m.name || "")}`}</div>
        <div class="kfoot">${chipOf(pick)}${reachOf(pick)}</div>
        <div class="kf-act">${getBtn(pick, true)}<button type="button" class="btn kr-ghost2" data-remix="${esc(keyOf(pick))}">${ico("spark")}Remix it</button></div></div>
        <div class="kf-r">${win(pick, true)}</div></div>`;
    }
    if (rest.length) html += `<div class="kr-kgrid">${rest.map(card).join("")}</div>`;
    else if (!pick) html += `<div class="kr-galempty"><b>${query ? `Nothing called “${esc(query)}” yet` : "Nothing published yet"}</b><span>${query ? "Try another word, or make it." : "Yours could be first."}</span>${query ? `<button type="button" class="btn kr-dark2" data-make>${ico("spark")}Make it</button>` : ""}</div>`;
    body.innerHTML = html;
    const mk = q("[data-make]", body);
    if (mk) mk.addEventListener("click", () => { const r = q('#side .side-row[data-side="home"]'); if (r) r.click(); setTimeout(() => { const ta = $("homePrompt"); if (ta) { ta.value = query; ta.dispatchEvent(new Event("input")); ta.focus(); } }, 300); });
    const tx = q(".kf-tx", body); if (tx && pick) typeSaid(tx, pick.meta.description);
  }
  // A thin bar in the home page's colours while something comes from the network.
  function topBar(on) {
    let b = q(".kr-tbar"); if (!b) { b = document.createElement("i"); b.className = "kr-tbar"; document.body.appendChild(b); }
    if (on) { b.style.transition = "none"; b.style.width = "0"; b.style.opacity = "1"; void b.offsetWidth; b.style.transition = "width .5s var(--ease)"; b.style.width = "30%"; setTimeout(() => { if (b.style.opacity === "1") { b.style.transition = "width 1.4s cubic-bezier(.1,.6,.3,1)"; b.style.width = "78%"; } }, 500); }
    else if (b.style.opacity === "1") { b.style.transition = "width .3s var(--ease), opacity .4s .25s"; b.style.width = "100%"; b.style.opacity = "0"; }
  }
  window.krTopBar = topBar;
  try {
    if (galView && typeof showCloudSkeleton === "function") {
      const sk = showCloudSkeleton; window.showCloudSkeleton = function () { sk(); skeleton(); };
      const rs = renderCloudShelves; window.renderCloudShelves = function (sh) { rs(sh); gal.shown = (sh || []).flatMap((x) => x.apps || []); paintGal(); };
      const rc = renderCloud; window.renderCloud = function (a, f) { rc(a, f); gal.shown = a || []; paintGal(); };
      const fc = typeof filterCloud === "function" ? filterCloud : null;
      if (fc) window.filterCloud = function () { fc(); paintGal(); };
      galShell();
      const errEl = $("cloudError");
      watch(errEl, { attributes: true, attributeFilter: ["class"], childList: true, characterData: true, subtree: true }, () => {
        if (!errEl.classList.contains("hidden") && errEl.textContent.trim()) {
          clearInterval(gal.lit); topBar(false);
          const body = q(".kr-galbody", galRoot);
          if (body && q(".kr-skc", body)) body.innerHTML = "";
        }
      });
    }
  } catch (e) {}

  /* ---- one gallery app, as the design has it ------------------------------ */
  (function galApp() {
    const view = $("viewApp"), main = view && q("main.detail", view);
    if (!main || typeof showCloudApp !== "function") return;
    view.classList.add("kr-kdon");
    const kd = document.createElement("div"); kd.className = "kr-kd";
    main.prepend(kd);
    let cur = null;
    const NOT = [[/^net\./, "No network"], [/^camera\./, "No camera"], [/^(fs\.|ui\.dialog:file|ui\.dialog:open)/, "None of your files"]];
    function caps(list) {
      const box = q(".kd-asks", kd); if (!box) return;
      let words = (c) => c; try { words = capWords; } catch (e) {}
      const yes = (list || []).map((c) => { try { return capWords(c); } catch (e) { return c; } }).filter((w, i, a) => w && a.indexOf(w) === i);
      const no = NOT.filter(([re]) => !(list || []).some((c) => re.test(String(c)))).map(([, w]) => w);
      box.innerHTML = yes.map((w, i) => `<span class="kr-ask y" style="--k:${i}"><i>${ico("check", "")}</i>${esc(w)}</span>`).join("") +
        no.map((w, i) => `<span class="kr-ask n" style="--k:${yes.length + i}"><i>${ico("no", "")}</i>${esc(w)}</span>`).join("");
    }
    function more(a) {
      const m = a.meta || {};
      const all = (gal.shown || []).filter((x) => x && x.meta && keyOf(x) !== keyOf(a));
      let list = m.author ? all.filter((x) => x.meta.author === m.author) : [];
      let head = m.author ? `More by @${esc(m.author)}` : "";
      if (!list.length) { const c = m.category; list = c ? all.filter((x) => x.meta.category === c) : []; head = c ? `More in ${esc(catName(c))}` : ""; }
      return list.length ? `<div class="kd-more"><h6>${head}</h6><div class="kr-kgrid">${list.slice(0, 3).map(card).join("")}</div></div>` : "";
    }
    function paint(a) {
      cur = a;
      const m = a.meta || {};
      const when = m.published ? (() => { try { return timeAgo(m.published); } catch (e) { return ""; } })() : "";
      kd.innerHTML = `<div class="kd-cols"><div class="kd-l">${win(a, true)}</div>
        <div class="kd-r"><h1>${esc(m.name || "Untitled app")}</h1>
          <div class="kd-by">${m.author ? `<span class="kr-av sm">${esc(m.author.charAt(0).toUpperCase())}</span>` : ""}${[m.author ? "@" + esc(m.author) : "", m.category ? esc(catName(m.category)) : "", a.opens > 0 ? `${a.opens} open${a.opens === 1 ? "" : "s"} this month` : ""].filter(Boolean).join(" · ")}</div>
          ${m.description ? `<div class="kd-said"><small>In one sentence</small><p>${esc(m.description)}</p></div>` : ""}
          <div class="kd-file"><img src="krate-doc.png" alt=""><div><b>${esc(fileOf(a))}</b><small>${[kb(a), when ? "published " + when : "", "macOS, Windows and Linux"].filter(Boolean).join(" · ")}</small></div></div>
          <div class="kd-act">${getBtn(a, true)}<button type="button" class="kr-ghost2" data-kd="run">${ico("play")}Run it</button><button type="button" class="kr-ghost2" data-remix="${esc(keyOf(a))}">${ico("spark")}Remix</button><button type="button" class="kr-ib" data-kd="link" title="Copy the link" aria-label="Copy the link">${ico("link")}</button></div>
          <div class="kd-note"></div>
          <div class="kd-sec"><h6>What it asks for</h6><div class="kd-asks"><span class="kd-dim">Reading the file…</span></div><p class="kd-wall">${ico("shield")}Krate enforces this list. The app cannot do anything that is not on it, whatever its code says.</p></div>
        </div></div>${more(a)}`;
      const note = $("detailNote"); if (note) q(".kd-note", kd).appendChild(note);
      if (Array.isArray(m.capabilities)) caps(m.capabilities);
      main.scrollTop = 0;
    }
    kd.addEventListener("click", (e) => {
      const k = e.target.closest("[data-kd]");
      if (k) { if (k.dataset.kd === "run") press($("detailRun")); else press($("detailCopy")); return; }
      const g = e.target.closest("[data-get]");
      if (g) { getFile(cur && keyOf(cur) === g.dataset.get ? cur : allApps().find((a) => keyOf(a) === g.dataset.get), g); return; }
      const r = e.target.closest("[data-remix]");
      if (r) { const a = cur && keyOf(cur) === r.dataset.remix ? cur : allApps().find((x) => keyOf(x) === r.dataset.remix); if (a) remix(a.meta.description || a.meta.name || "", a.meta.name || "this app"); return; }
      const c = e.target.closest("[data-app]");
      if (c) { const a = allApps().find((x) => keyOf(x) === c.dataset.app); if (a) { try { showCloudApp(a); } catch (err) {} } }
    });
    const orig = showCloudApp;
    window.showCloudApp = function (a) { orig(a); try { paint(a); } catch (e) {} };
    const rcg = renderCapGroups;
    window.renderCapGroups = function (host, list) { rcg(host, list); if (host && host.id === "detailCaps") caps(list); };
  })();

  /* ---- loaders: lists, the network, opening an app, saving ---------------- */
  // A pill at the foot of the window: offline and back, an app opening.
  function pill(id) {
    let n = $(id);
    if (!n) { n = document.createElement("div"); n.id = id; n.className = "kr-netp"; n.setAttribute("role", "status"); n.innerHTML = `${window.krIso ? window.krIso(30) : ""}<span class="kr-swp"><span></span></span>`; document.body.appendChild(n); }
    return n;
  }
  // Offline: the layers slip out of line and wobble; back online they click
  // into place and turn blue again.
  (function net() {
    let hideT = 0;
    const off = () => { const n = pill("krNet"); clearTimeout(hideT); n.classList.add("off", "on"); q(".kr-swp", n).innerHTML = "<span><b>You're offline</b><small>Your apps still open. Trying again…</small></span>"; };
    const back = () => { const n = $("krNet"); if (!n || !n.classList.contains("on")) return; n.classList.remove("off"); window.krSwap(q(".kr-swp", n), "<b>Back online</b><small>Everything is in sync.</small>"); hideT = setTimeout(() => n.classList.remove("on"), 2400); };
    addEventListener("offline", off); addEventListener("online", back);
    if (navigator.onLine === false) off();
  })();
  // Opening an app: the top layer lifts like a lid, and the line says how
  // long it really took.
  try {
    if (typeof openApp === "function") {
      const oa = openApp;
      window.openApp = async function (which, version) {
        let a = null; try { a = (which && which.path) ? which : currentApp(); } catch (e) {}
        const name = a ? (a.name || "").replace(/\.krate$/, "").replace(/[-_]+/g, " ").replace(/\b\w/g, (c) => c.toUpperCase()) : "your app";
        const n = pill("krOpen"); n.classList.remove("off", "lift"); n.classList.add("on", "opening");
        q(".kr-swp", n).innerHTML = `<span><b>Opening ${esc(name)}</b></span>`;
        setTimeout(() => n.classList.add("lift"), 380);
        const t0 = performance.now();
        const before = (($("composerHint") || {}).textContent || "");
        try { await oa(which, version); } finally {
          const secs = ((performance.now() - t0) / 1000).toFixed(1);
          const failed = (($("composerHint") || {}).textContent || "") !== before;
          window.krSwap(q(".kr-swp", n), failed ? `<b>${esc(name)} did not open</b><small>The reason is under the box.</small>` : `<b>${esc(name)}</b><small>${+secs >= 0.1 ? `opened in ${secs} s` : "opened"}</small>`);
          setTimeout(() => n.classList.remove("on", "opening", "lift"), 2600);
        }
      };
    }
  } catch (e) {}
  // Your apps, the first time: cards shaped like the real ones with a quiet
  // breathing mark -- only when the list takes longer than 0.4 s.
  try {
    if (typeof loadAppsPage === "function") {
      const lp = loadAppsPage;
      window.loadAppsPage = async function () {
        const grid = $("appsGrid");
        const t = setTimeout(() => {
          if (!grid || q(".app-card", grid)) return;
          grid.innerHTML = Array.from({ length: 4 }, (_, i) => `<div class="kr-skapp" style="--k:${i}"><div class="th">${window.krIso ? window.krIso(40, "breathe", true) : ""}</div><span class="l1"></span><span class="l2"></span></div>`).join("");
        }, 400);
        try { return await lp(); } finally { clearTimeout(t); qa(".kr-skapp", grid || document).forEach((x) => x.remove()); }
      };
    }
  } catch (e) {}
  // Saving the name: an orange dot beats, then grows into a green circle
  // and a tick draws itself. (The name is written as you type.)
  (function saving() {
    const nick = $("profNick"); if (!nick) return;
    const sv = document.createElement("span"); sv.className = "kr-svs";
    sv.innerHTML = '<span class="sdot"><i></i><svg viewBox="0 0 14 14"><circle cx="7" cy="7" r="7"/><path pathLength="1" d="M4 7.3l2 2 4-4.3"/></svg></span><span class="kr-swp"><span></span></span>';
    nick.after(sv);
    let t1 = 0, t2 = 0;
    nick.addEventListener("input", () => {
      sv.classList.remove("saved"); sv.classList.add("saving"); window.krSwap(q(".kr-swp", sv), "Saving");
      clearTimeout(t1); clearTimeout(t2);
      t1 = setTimeout(() => { sv.classList.remove("saving"); sv.classList.add("saved"); window.krSwap(q(".kr-swp", sv), "Saved"); t2 = setTimeout(() => { sv.classList.remove("saved"); window.krSwap(q(".kr-swp", sv), ""); }, 1800); }, 500);
    });
  })();

  /* ---- the sign-in page ---------------------------------------------------- */
  // A page of its own: no workspace beside it, the mark stacking in, and a
  // breathing mark while the browser does the signing in.
  (function gate() {
    const g = $("viewGate"); if (!g) return;
    g.classList.add("kr-gate");
    const mark = q(".gate-mark", g);
    if (mark && window.krIso) { const sp = document.createElement("span"); sp.className = "kr-gmark"; sp.innerHTML = window.krIso(56, "stack"); mark.replaceWith(sp); }
    const h = q("h1", g); if (h) h.textContent = "Sign in to Krate";
    const sub = q(".gate-sub", g); if (sub) sub.textContent = "To publish your apps and keep them with your account. Making apps works without it.";
    const wait = document.createElement("div"); wait.className = "kr-gwait"; wait.hidden = true;
    wait.innerHTML = `${window.krIso ? window.krIso(30, "breathe") : ""}<span><b>Finish in your browser</b><small>This page moves on by itself once you have signed in.</small></span>`;
    const start = $("gateStart"); if (start) start.after(wait);
    const btn = $("loginBrowserBtn");
    if (btn) btn.addEventListener("click", () => { wait.hidden = false; }, true);
    const err = $("gateError");
    if (err) watch(err, { attributes: true, attributeFilter: ["class"] }, () => { if (!err.classList.contains("hidden")) wait.hidden = true; });
    const sync = () => {
      const on = !g.classList.contains("hidden");
      document.body.classList.toggle("kr-gating", on);
      if (on && window.krStack) { qa(".kr-gmark .ly", g).forEach((l) => l.classList.remove("on")); window.krStack(g, 200); }
      if (!on) wait.hidden = true;
    };
    watch(g, { attributes: true, attributeFilter: ["class"] }, sync); sync();
  })();

  /* ---- the sidebar's Port an app ---------------------------------------- */
  const sidePort = $("sidePort");
  if (sidePort) sidePort.addEventListener("click", () => {
    const homeRow = q('#side .side-row[data-side="home"]');
    if (homeRow && $("viewHome") && $("viewHome").classList.contains("hidden")) homeRow.click();
    setMode("port");
    startPort();
  });
})();
