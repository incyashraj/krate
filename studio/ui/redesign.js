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
      b.innerHTML = (it.icon ? ico(it.icon) : "") +
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
    bar.dataset.mode = mode;
    if (pane) pane.hidden = mode !== "port";
    const kb = $("homeKb"); if (kb) kb.hidden = mode === "port";
    placeInd();
    const sub = $("homeSub");
    if (sub) {
      sub.textContent = mode === "port" ? "Krate reads it first and tells you what the port would take."
        : "Describe it. Krate turns it into one small file that opens on every desktop.";
      sub.classList.remove("kr-swap"); void sub.offsetWidth; sub.classList.add("kr-swap");
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

  /* ---- the AI's own mark wherever the AI is named ---------------------- */
  function agentMark(name) {
    const n = String(name || "").toLowerCase();
    if (/claude|anthropic/.test(n)) return ico("claude", "kr-lgo");
    if (/codex|openai|gpt/.test(n)) return ico("openai", "kr-lgo");
    if (/gemini|google/.test(n)) return ico("gemini", "kr-lgo");
    if (/krate/.test(n)) return '<img class="kr-lgo" src="krate-logo.png" alt="">';
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
    if (kbd) kbd.textContent = navigator.platform && /mac/i.test(navigator.platform) ? "⌘K" : "Ctrl K";
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

  /* ---- search everything: ⌘K ------------------------------------------- */
  (function palette() {
    const scrim = document.createElement("div"); scrim.className = "kr-scrim";
    const pal = document.createElement("div"); pal.className = "kr-pal"; pal.setAttribute("role", "dialog"); pal.setAttribute("aria-label", "Search");
    pal.innerHTML = `<div class="kr-pin">${ico("search")}<input type="text" placeholder="Search your apps and Studio" aria-label="Search" autocomplete="off" spellcheck="false"><kbd>esc</kbd></div><div class="kr-pres"></div><div class="kr-pf"><span>↑↓ to move</span><span>↵ to open</span><span>⌘K anywhere</span></div>`;
    document.body.append(scrim, pal);
    const input = q("input", pal), res = q(".kr-pres", pal);
    let items = [], hl = 0;
    const isOn = () => pal.classList.contains("on");
    const rowClick = (sel) => () => { const r = q(sel); if (r) r.click(); };
    function render() {
      const s = input.value.trim().toLowerCase();
      const dark = !document.body.classList.contains("light");
      const acts = [
        ["New app", "new", rowClick('#side .side-row[data-side="home"]')],
        ["Port an app", "port", rowClick("#sidePort")],
        ["Your apps", "grid", rowClick('#side .side-row[data-side="all"]')],
        ["Gallery", "compass", rowClick('#side .side-row[data-side="discover"]')],
        ["Settings", "gear", () => press($("sideSettings"))],
        [dark ? "Switch to light" : "Switch to dark", dark ? "sun" : "moon", () => press($("themeBtn"))],
      ].filter((a) => a[0].toLowerCase().includes(s)).map((a) => ({ g: "Actions", t: a[0], i: ico(a[1]), f: a[2] }));
      const sess = qa("#sideSessions .sess-row").map((r) => ({ r, t: (q(".sess-name", r) || r).textContent.trim() }))
        .filter((x) => x.t.toLowerCase().includes(s)).slice(0, 8)
        .map((x) => ({ g: "Your apps", t: x.t, i: ico("file"), f: () => x.r.click() }));
      items = [...acts, ...sess];
      if (s && !items.length) items = [{ g: "Make it", t: `Make “${input.value.trim()}”`, i: ico("spark"), f: () => {
        rowClick('#side .side-row[data-side="home"]')();
        const ta = $("homePrompt"); if (ta) { ta.value = input.value.trim(); ta.dispatchEvent(new Event("input")); ta.focus(); }
      } }];
      hl = Math.max(0, Math.min(hl, items.length - 1));
      let g = "";
      res.innerHTML = items.map((it, i) => (it.g !== g ? `<div class="kr-ph2">${esc((g = it.g))}</div>` : "") +
        `<button type="button" class="kr-pi${i === hl ? " hl" : ""}" data-i="${i}">${it.i}<span class="kr-pt">${esc(it.t)}</span></button>`).join("");
    }
    function open() { closePop(); input.value = ""; hl = 0; render(); scrim.classList.add("on"); pal.classList.add("on"); setTimeout(() => input.focus(), 30); }
    function close() { scrim.classList.remove("on"); pal.classList.remove("on"); }
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
    // On the icon rail the search row is only an icon: it opens this.
    const ss = q(".side-search");
    if (ss) ss.addEventListener("click", (e) => { if (document.body.classList.contains("kr-rail")) { e.preventDefault(); open(); } });
    document.addEventListener("keydown", (e) => {
      if ((e.metaKey || e.ctrlKey) && !e.altKey && e.key.toLowerCase() === "k") { e.preventDefault(); isOn() ? close() : open(); }
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
  watch(thread, { childList: true }, (recs) => {
    const added = [];
    recs.forEach((r) => r.addedNodes.forEach((n) => { if (n.nodeType === 1) added.push(n); }));
    if (added.length > 2) return;
    added.forEach((n) => {
      n.classList.add("kr-in");
      if (n.matches(".msg.krate") && !n.matches(".vlive, .vok, .vbad")) wordIn(n);
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
  setInterval(paintTools, 400);
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
      const have = qa("li.no", asks).map((li) => li.textContent);
      if (have.length === nots.length && have.every((t, i) => t === nots[i])) return;
      qa("li.no", asks).forEach((li) => li.remove());
      nots.forEach((t) => { const li = document.createElement("li"); li.className = "no"; li.textContent = t; asks.appendChild(li); });
      qa("li", asks).forEach((li, i) => li.style.setProperty("--k", i));
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
  // Published: the sheet closes and the link is on the clipboard. Say so
  // where it is seen. Only when a NEW link appeared, so closing the sheet
  // any other way says nothing.
  (function published() {
    const sheet = $("publishSheet");
    if (!sheet) return;
    let before = null;
    const link = () => { const s = app(); return s && s.session && s.session.result ? s.session.result.share_url || null : null; };
    watch(sheet, { attributes: true, attributeFilter: ["class"] }, () => {
      if (!sheet.classList.contains("hidden")) { before = link(); return; }
      const now = link();
      if (now && now !== before) toast("Published. The link is copied", true);
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
    const ICONS = { agent: "cpu", output: "folder", updates: "refresh", gallery: "compass", support: "msg", privacy: "shield",
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

  /* ---- the sidebar's Port an app ---------------------------------------- */
  const sidePort = $("sidePort");
  if (sidePort) sidePort.addEventListener("click", () => {
    const homeRow = q('#side .side-row[data-side="home"]');
    if (homeRow && $("viewHome") && $("viewHome").classList.contains("hidden")) homeRow.click();
    setMode("port");
    startPort();
  });
})();
