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
    let x = Math.min(Math.max(8, opts.right ? r.right - w : r.left), innerWidth - w - 8);
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
    // A view counts once it can be SEEN: the web keeps the sign-in page
    // invisible while it asks the hub who this is, and the loader left on
    // that, over a blank page, for as long as the hub took.
    const ready = () => ["viewHome", "viewSession", "viewGate", "viewApps", "viewCloud", "viewIde"].some((id) => { const v = $(id); return v && !v.classList.contains("hidden") && getComputedStyle(v).visibility !== "hidden"; });
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
    const ideRow = $("sideIde"), homeRow = q('#side .side-row[data-side="home"]'), portRow = $("sidePort");
    if (homeRow && !$("viewHome").classList.contains("hidden")) {
      if (ideRow) ideRow.classList.toggle("on", mode === "ide");
      if (portRow) portRow.classList.toggle("on", mode === "port");
      homeRow.classList.toggle("on", mode === "create");
    }
    // A note under the box belongs to the door it was written for; a port
    // or IDE message must not stay under Create (or the other way round).
    const hh = $("homeHint"); if (hh && hh.dataset.mode && hh.dataset.mode !== mode) { hh.textContent = ""; }
    if (hh) hh.dataset.mode = mode;
    if (mode === "create") { const ta = $("homePrompt"); if (ta) ta.focus({ preventScroll: true }); }
  }
  if (tabs) {
    tabs.addEventListener("click", (e) => { const b = e.target.closest("button[data-mode]"); if (b && !b.classList.contains("on")) setMode(b.dataset.mode); });
    // The IDE's Back and the sidebar doors pick a tab through here.
    window.krHomeMode = (m) => setMode(m);
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
    // The mark follows the AI that is chosen (state.agent), not the words on
    // the chip: "engine trouble" or "no AI found" once painted a bare green
    // dot where the session bar, which hides the words, showed nothing else.
    // A trouble label keeps the logo and adds an amber corner; "…" (still
    // looking) breathes the Krate mark.
    const paint = () => {
      const k = nameEl.textContent.trim();
      const st = app(); const id = (st && st.agent) || "";
      const looking = !k.replace(/[.…\s]/g, "");
      const key = looking ? "…" : id + "|" + k;
      if (lg.dataset.k === key) return;
      lg.dataset.k = key;
      const known = /claude|anthropic|codex|openai|gpt|gemini|google|krate/i.test(k);
      lg.innerHTML = looking ? (window.krIso ? window.krIso(15, "breathe", true) : "")
        : known ? agentMark(k)
        : id ? (/claude|anthropic|codex|openai|gpt|gemini|google|krate/.test(id) ? agentMark(id) : (() => { try { return aiLogo(id); } catch (e) { return agentMark(id); } })())
        : agentMark(k);
      chip.classList.toggle("kr-aibad", !looking && /trouble|no AI|not installed|needs a fix|sign in|usage limit|paused/i.test(k));
    };
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
      const kick = () => {
        if (!field.value.trim() || send.classList.contains("kr-voice") || send.classList.contains("stopping")) return;
        send.classList.remove("kr-sending"); void send.offsetWidth;
        send.classList.add("kr-sending");
        clearTimeout(send._sdT); send._sdT = setTimeout(() => send.classList.remove("kr-sending"), 800);
      };
      send.addEventListener("click", kick, true);
      // Enter sends too; app.js clears the box in its own handler, so read it first.
      window.addEventListener("keydown", (e) => {
        if (e.target === field && e.key === "Enter" && !e.shiftKey && !e.isComposing) kick();
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
        { icon: dark ? "sun" : "moon", label: dark ? "Light mode" : "Dark mode", run: () => (window.krSetTheme ? window.krSetTheme(dark ? "light" : "dark") : press($("themeBtn"))) },
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
    let items = [], hl = 0, mine = [], drafts0 = [];
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
      drafts0 = list.filter((x) => !(x.result && x.result.path)).sort((a, b) => (b.updated || 0) - (a.updated || 0));
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
        ["Port an app", "port", rowClick("#sidePort"), "Beta"],
        ["Open the IDE", "code", rowClick("#sideIde"), "Beta"],
        ["Settings", "gear", () => press($("sideSettings")), mod + ","],
        [dark ? "Switch to light" : "Switch to dark", dark ? "sun" : "moon", () => (window.krSetTheme ? window.krSetTheme(dark ? "light" : "dark") : press($("themeBtn"))), ""],
        ["Gallery", "compass", rowClick('#side .side-row[data-side="discover"]'), ""],
      ].filter((a) => (a[0] !== "Open the IDE" || $("sideIde")) && a[0].toLowerCase().includes(s))
        .map((a) => ({ g: "Actions", t: a[0], i: ico(a[1]), k: a[3], f: a[2] }));
      const apps = mine.filter((x) => appName(x).toLowerCase().includes(s) || (x.title || "").toLowerCase().includes(s)).slice(0, 8)
        .map((x) => ({ g: "Your apps", t: appName(x), i: `<span class="kr-th0">${shotOf.has(x.id) ? `<img src="${esc(shotOf.get(x.id))}" alt="">` : ""}</span>`, k: x.result.size || "", f: () => { try { openSession(x); } catch (e) {} } }));
      const st = app();
      const gal = s && st && Array.isArray(st.cloud) ? st.cloud.filter((a) => ((a.meta && a.meta.name) || "").toLowerCase().includes(s)).slice(0, 5)
        .map((a) => ({ g: "Gallery", t: a.meta.name, i: `<span class="kr-th0">${a.shot ? `<img src="${esc(a.shot)}" alt="">` : ""}</span>`, k: a.meta.author ? "@" + a.meta.author : "", f: () => { try { showCloudApp(a); } catch (e) {} } })) : [];
      // Drafts too: a session that never built is still the person's work,
      // and past Recents' first rows search was the only way back to it.
      const drafts = s ? drafts0.filter((x) => (x.title || "").toLowerCase().includes(s)).slice(0, 6)
        .map((x) => ({ g: "Drafts", t: x.title || "Untitled", i: ico("pencil"), k: "draft", f: () => { try { openSession(x); } catch (e) {} } })) : [];
      items = [...acts, ...apps, ...drafts, ...gal];
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
    let plan = false; try { plan = isPlanning(); } catch (e) {}
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
    // The built app's own name first, then the name its plan gave it.
    const st = app(), sess = st && st.session;
    const built = sess && sess.result && sess.result.name ? String(sess.result.name).replace(/\.krate$/, "").replace(/[-_]+/g, " ").replace(/\b\w/g, (c) => c.toUpperCase()) : "";
    if (built) return built;
    const planned = planName();
    if (planned) return planned;
    const t = (($("railTitle") || {}).textContent || "").trim();
    const first = q(".msg.you .body", thread);
    const asked = first ? first.textContent.trim() : "";
    if (t && t !== "New app" && t !== asked) return t;
    // A request reads as a sentence, often after a greeting: "hi, lets make
    // a chess game". The name is the thing after the verb; where there is
    // no clear thing, it is just "Your app", never the greeting.
    let p = asked.replace(/^(?:(?:hi|hey|hello|yo|ok|okay|so|please|pls)\b[\s,!.]*)+/i, "");
    p = p.replace(/^(?:(?:can|could|would|will) you\s+|i\s+(?:want|need|would like)(?: you)?(?: to)?\s+|let'?s\s+|lets\s+)/i, "");
    p = p.replace(/^(?:please\s+)?(?:make|build|create|write|do|code|give)\s+(?:me\s+|us\s+)?/i, "").replace(/^(?:something\s+like\s+)?(?:a|an|the|my|some)\s+/i, "");
    const w = p.split(/\s+(?:with|that|for|which|to|where|using|so|in|on|and)\s+/i)[0].split(/\s+/).slice(0, 3).join(" ").replace(/[.,;:!?]+$/, "");
    if (!w || /^(?:a|an|the|app|something|thing|it)$/i.test(w) || w.length > 28) return "Your app";
    return w.replace(/\b\w/g, (c) => c.toUpperCase());
  }
  // The name the plan gave the app ("Tip Split"), while that plan is live.
  function planName() { const st = app(); return (st && st.planning && st.planning.name) || (st && st.session && st.session.planName) || ""; }
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
      // The engine's short points come as "• " lines under one sentence;
      // the bullets alone are the card, as the design has it. An older engine
      // sends sentences only.
      const pts = (text.match(/^•\s*(.+)$/gm) || []).map((l) => l.replace(/^•\s*/, "").trim());
      if (pts.length) items = pts;
      else items = sentences(text);
    }
    const card = document.createElement("div");
    card.className = "kr-plan";
    card.innerHTML = `<span class="kr-ptag">${ico("spark", "kr-ico")}${port ? "Port plan" : "Plan"}</span><h4>${esc(planName() || appName())}</h4>` +
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
      // With apps, and also on the first visit's empty page: an empty grid
      // had only a sentence, with no way to start from it.
      const empty = q(".apps-empty", grid) && /Nothing yet/.test(q(".apps-empty", grid).textContent);
      if (q(".kr-newcard", grid) || !(q(".app-card", grid) || empty)) return;
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
      const go = (b) => { card.click(); thenPress(b.dataset.a === "run" ? "barRun" : "barShare"); };
      h.addEventListener("click", (e) => {
        const b = e.target.closest("[data-a]"); if (!b) return;
        e.stopPropagation(); e.preventDefault();
        go(b);
      });
      // By keyboard too: Enter or Space on Run or Share does that, and does
      // not fall through to the card (which only opened the session).
      h.addEventListener("keydown", (e) => {
        const b = e.target.closest("[data-a]"); if (!b || (e.key !== "Enter" && e.key !== " ")) return;
        e.stopPropagation(); e.preventDefault();
        go(b);
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
      if (!order.length) { box.innerHTML = `<p class="kr-agc-empty">${st.agentsError ? "Krate could not look for AI tools on this computer." : st.agentsChecked ? "No AI tool is installed on this computer yet. Install one, or add an API key." : "Looking for AI tools on this computer…"}</p>`; return; }
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
    // "Add a key" opens the sheet on its API key tab (it opened on
    // Installed tools, one more click from what was asked for).
    if (keyBtn) keyBtn.addEventListener("click", (e) => {
      e.stopImmediatePropagation(); sheet();
      setTimeout(() => { const t = q('#aiNav [data-ai="keys"]'); if (t) press(t); }, 0);
    }, true);
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
      // Signed out: the popup asks, and when it lands this button makes the
      // link again, right here (as Publish does).
      window.__krPubAgain = () => { delete b.dataset.busy; b.classList.remove("busy"); q(".kr-lb", b).textContent = "Make a link"; b.click(); };
      try {
        openPublishSheet();
        if (sheet) sheet.classList.add("hidden");
        const listed = $("pubListed"); if (listed) listed.checked = false;
        await publishFromSheet();
      } catch (e) { /* app.js says why in the publish sheet's note */ }
      linking = false;
      const url = linkOf();
      if (url) { window.__krPubAgain = null; paintLink(url, true); toast("Link made and copied", true); return; }
      // Waiting on the sign-in popup, already open over this pane; the old
      // publish sheet stays shut (it was left behind on its sign-in step).
      if ($("pubSignin") && !$("pubSignin").classList.contains("hidden")) {
        if (sheet) sheet.classList.add("hidden");
        delete b.dataset.busy; b.classList.remove("busy"); q(".kr-lb", b).textContent = "Make a link";
        return;
      }
      window.__krPubAgain = null;
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
      if (!n.value.trim()) { n.focus(); err.textContent = "Give it a name first."; err.hidden = false; return; }
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
      // Signed out: the popup asks, and when it lands this same button
      // publishes again here, ring and stamp and all (it finished behind the
      // old sheet with only a toast).
      window.__krPubAgain = () => { delete go.dataset.busy; q("span", go).textContent = "Publish"; go.click(); };
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
      if (url && !signIn) { window.__krPubAgain = null; return published(pane, n.value.trim(), url, listed); }
      // Waiting on the sign-in popup, which is already open over this pane.
      if (signIn) { if (sheet) sheet.classList.add("hidden"); delete go.dataset.busy; q("span", go).textContent = "Publish"; return; }
      window.__krPubAgain = null;
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
  // The welcome, as the first design had it: the mark, one line, one
  // sentence, and a small Studio playing the whole thing once round --
  // a request typed, the crate made, the real app it became.
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
    <div class="kr-ob-f"><button type="button" class="kr-plain kr-ob-skip">Skip</button><span class="kr-dots"><i class="on"></i><i></i><i></i></span><button type="button" class="kr-dark kr-ob-go">Take the tour</button></div>
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
  // The dots follow the preview: one per scene (four dots that never moved
  // promised pages that were not there).
  const pvDot = (i) => qa(".kr-ob-f .kr-dots i", obWrap).forEach((d, k) => d.classList.toggle("on", k === i));
  async function pvPlay() {
    const my = ++pvRun, pv = q(".kr-pv", obWrap), tx = q(".kr-pv-tx", obWrap), s = "a weather app for my cities";
    if (reduce) { pv.dataset.ph = "c"; pvDot(2); return; }
    while (my === pvRun && obWrap.classList.contains("on")) {
      pv.dataset.ph = "a"; pvDot(0); tx.textContent = "";
      for (let i = 1; i <= s.length; i++) { tx.textContent = s.slice(0, i); await sleep(42); if (my !== pvRun) return; }
      await sleep(500); if (my !== pvRun) return;
      pv.dataset.ph = "b"; pvDot(1); await sleep(2600); if (my !== pvRun) return;
      pv.dataset.ph = "c"; pvDot(2); await sleep(3000);
    }
  }
  // On the web a note about what is free also opens on a first visit.
  // One thing at a time: it waits until the welcome and the tour are done.
  let heldNote = null;
  const holdNote = () => { const n = $("welcomeSheet"); if (n && !n.classList.contains("hidden")) { n.classList.add("hidden"); heldNote = n; } };
  const releaseNote = () => { if (heldNote) { heldNote.classList.remove("hidden"); heldNote = null; return true; } return false; };
  function obOpen() {
    // After Studio's opening, not under it, whichever door asked.
    const boot = q(".kr-boot");
    if (boot && !boot.classList.contains("gone")) { setTimeout(obOpen, 150); return; }
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
      // After Studio's opening, not under it: the welcome's entrance played
      // unseen behind the loader (the design opens it once the loader goes).
      const boot = q(".kr-boot"), booting = boot && !boot.classList.contains("gone");
      if (home && !home.classList.contains("hidden") && !busy && !booting) { clearInterval(t); obOpen(); }
      else if (++tries > 80) clearInterval(t);
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

  /* ---- the right side: one card, start to finish -------------------------- */
  // The design's card. A quiet grey inset with one glyph per step (thinking,
  // writing, building, checking, packing), one line under it that says what
  // Krate is doing to this app, and the time. When the app is made it
  // arrives in the inset, small, then settles into the card as the preview,
  // with the file, Run and Share underneath and what it may do said plainly.
  // A failure keeps the card, edged red, with what happened and what to do.
  //
  // app.js still drives its own state panes (statePlanning, stateBuilding,
  // stateDone, stateFailed) and owns every action; this card reads them and
  // presses their buttons. The panes stay in the page, out of sight.
  (function forge() {
    const stage = $("stage");
    if (!stage) return;
    const P = (i, d, extra) => `<path class="${extra || "ol"}" pathLength="1" style="--i:${i}" d="${d}"/>`;
    const spokes = [0, 60, 120, 180, 240, 300].map((r, i) => `<path class="sp" pathLength="1" style="--i:${i}" transform="rotate(${r} 50 50)" d="M50 50 L50 14"/>`).join("");
    const GL = {
      think: `<g class="rot">${spokes}</g>`,
      write: `<g class="nud">${P(0, "M30 30 L51 50 L30 70")}</g><rect class="cur" x="57" y="64.5" width="16" height="5.5" rx="2.4"/>`,
      build: `<g class="bob">${P(0, "M50 17 L82 33 L50 49 L18 33 Z")}${P(1, "M18 33 V67 L50 83 L82 67 V33")}${P(2, "M50 49 V83")}${P(0, "M18 44.5 L50 60.5 L82 44.5", "ly")}${P(1, "M18 56 L50 72 L82 56", "ly")}</g>`,
      check: `<circle class="trk" cx="50" cy="50" r="34"/><circle class="arc" pathLength="1" cx="50" cy="50" r="34"/>${P(1, "M37 51 L46 60 L64 41", "ol tk")}`,
      pack: `<g class="bob">${P(0, "M33 15 H58 L71 28 V85 H33 Z")}${P(1, "M58 15 V28 H71")}${P(0, "M42 46 H62", "ln")}${P(1, "M42 56.5 H62", "ln")}${P(2, "M42 67 H54", "ln")}</g>`,
    };
    const ast = `<svg class="kr-ast" width="14" height="14" viewBox="0 0 100 100" aria-hidden="true"><g>${[0, 60, 120, 180, 240, 300].map((r) => `<path transform="rotate(${r} 50 50)" d="M50 50 L50 12"/>`).join("")}</g></svg>`;
    const f = document.createElement("div");
    f.className = "kr-forge"; f.dataset.st = "idle"; f.dataset.ph = "";
    f.innerHTML =
      `<div class="kr-fcard">` +
        `<div class="kr-fth"><div class="kr-fgl" aria-hidden="true">${Object.entries(GL).map(([k, g]) => `<svg class="kr-gl" data-g="${k}" viewBox="0 0 100 100">${g}</svg>`).join("")}</div>` +
          `<div class="kr-flive"><img alt="Your app, as it renders"></div>` +
          `<div class="kr-flg"><div class="kr-flg-in" tabindex="0" aria-label="What Krate is doing, line by line"></div><button type="button" class="kr-flg-copy">Copy</button></div></div>` +
        `<div class="kr-frows">` +
          `<div class="kr-fmeta"><span class="kr-fst" aria-live="polite"><span></span></span><span class="kr-fmk">${ast}</span><span class="kr-grow"></span><span class="kr-fel"></span></div>` +
          `<div class="kr-fdone"><img class="kr-dcic" src="krate-doc.png" alt=""><div class="kr-dn"><b></b><small></small></div>` +
            `<div class="kr-da"><button type="button" class="btn kr-dark" data-fa="run">${ico("play", "kr-ico")}Run</button><button type="button" class="btn kr-ghost" data-fa="share">${ico("share", "kr-ico")}Share</button><button type="button" class="kr-fmore" data-fa="more" title="More">${ico("more", "kr-ico")}</button></div></div>` +
        `</div>` +
        `<div class="kr-fx kr-fverd"><div><p></p></div></div>` +
        `<div class="kr-fx kr-fasks"><div><p class="kr-perm"></p></div></div>` +
        `<div class="kr-fx kr-fnote"><div><p class="kr-fstill">${ast}Still going. Bigger apps take a minute.</p></div></div>` +
      `</div>` +
      `<div class="kr-ffail"><h5><i>!</i><span></span></h5><p></p><div class="kr-facts2"></div></div>` +
      `<div class="kr-facts"><button type="button" class="btn kr-plain" data-fa="log">${ico("term", "kr-ico")}<span>Show details</span></button><button type="button" class="btn kr-plain" data-fa="stop">${ico("stop", "kr-ico")}Stop</button></div>`;
    stage.insertBefore(f, stage.firstChild);
    stage.classList.add("kr-forged");
    const fst = q(".kr-fst", f), fel = q(".kr-fel", f), img = q(".kr-flive img", f);
    // Details: the engine's own lines, quietly, inside the card's grey
    // panel where the glyph was -- small and light, the newest at the
    // bottom, the oldest fading out at the top, each step marked with a dot.
    // Read from app.js's log (buildLog), which keeps every line.
    const lg = q(".kr-flg-in", f), lgLog = $("buildLog");
    let logOn = false; try { logOn = localStorage.getItem("kr-forge-details") === "1"; } catch (e) {}
    let lgKey = "", lgQ = 0;
    function paintLog() {
      lgQ = 0;
      const text = (lgLog && lgLog.textContent) || "";
      const lines = text.replace(/\n+$/, "").split("\n").filter((l) => l.trim()).slice(-80);
      const key = lines.length + "|" + (lines[lines.length - 1] || "");
      if (key === lgKey) return;
      const atEnd = lg.scrollHeight - lg.scrollTop - lg.clientHeight < 24;
      const had = lg.children.length && lgKey ? lg.querySelectorAll(".kr-ll").length : 0;
      lgKey = key; lg.innerHTML = "";
      if (!lines.length) { lg.innerHTML = '<div class="kr-ll kr-lq">Nothing yet. Lines appear here as Krate works.</div>'; return; }
      lines.forEach((l, i) => {
        const d = document.createElement("div");
        const step = /^=+>/.test(l), bad = /\b(error|failed|refused|panicked)\b/i.test(l);
        d.className = "kr-ll" + (step ? " kr-ls" : "") + (bad ? " kr-le" : "") + (i >= had && had ? " kr-lnew" : "");
        d.textContent = step ? l.replace(/^=+>\s*/, "") : l.replace(/\t/g, "  ");
        lg.appendChild(d);
      });
      if (atEnd || !had) lg.scrollTop = lg.scrollHeight;
    }
    if (lgLog) watch(lgLog, { childList: true, characterData: true, subtree: true }, () => { if (!lgQ) lgQ = requestAnimationFrame(paintLog); });
    q(".kr-flg-copy", f).addEventListener("click", (e) => {
      const b = e.currentTarget;
      press($("termCopy"));
      b.textContent = "Copied"; setTimeout(() => { b.textContent = "Copy"; }, 1400);
    });

    /* the glyph: one at a time, each shown long enough to be seen */
    let glT = 0, glQ = 0;
    function glyph(name) {
      const on = q(".kr-gl.on", f);
      if (on && on.dataset.g === name) { clearTimeout(glQ); return; }
      clearTimeout(glQ);
      const set = () => { glT = Date.now(); qa(".kr-gl", f).forEach((g) => g.classList.toggle("on", g.dataset.g === name)); };
      const since = Date.now() - glT;
      if (since < 1400) glQ = setTimeout(set, 1400 - since); else set();
    }
    const GLYPH = { "": "think", plan: "think", write: "write", build: "build", look: "check", pack: "pack", wall: "check" };
    const WORD = { "": ["thinking about", "var(--accent)"], plan: ["planning", "var(--accent)"], write: ["writing", "var(--violet)"], build: ["building", "var(--accent)"], look: ["checking", "var(--violet)"], pack: ["packing", "var(--orange)"], wall: ["still checking", "var(--violet)"] };

    /* the line under the inset */
    function plain(t, cls) {
      fst.dataset.k = "";
      const s = q(":scope > span", fst);
      const head = (x) => x.split(" · ")[0];
      if (s && !s.classList.contains("kr-fsnt") && head(s.textContent) === head(t) && (s.className || "") === (cls || "")) { s.textContent = t; return; }
      fst.innerHTML = ""; const n = document.createElement("span"); if (cls) n.className = cls; n.textContent = t; fst.appendChild(n);
    }
    // The sentence is written once; after that only its word turns over.
    function sentence(ph) {
      const nm = appName();
      const name = !nm || nm === "Your app" ? "app" : nm.toLowerCase();
      if (fst.dataset.k === ph + "|" + name) return;
      fst.dataset.k = ph + "|" + name;
      // A port is "porting" while it writes; every other step is the same.
      const st0 = app(), porting = !!(st0 && st0.session && st0.session.portSource && !(st0.session.result && st0.session.result.path));
      const w = porting && (ph === "write" || ph === "plan" || ph === "") ? ["porting", "var(--violet)"] : WORD[ph] || WORD.write;
      let fw = q(".kr-fw", fst);
      if (!fw || fst.dataset.name !== name) {
        fst.dataset.name = name;
        fst.innerHTML = `<span class="kr-fsnt">Krate is <span class="kr-fw"></span> your ${esc(name)}</span>`;
        fw = q(".kr-fw", fst);
      }
      const old = q("span.on", fw), n = document.createElement("span");
      n.textContent = w[0]; n.style.setProperty("--c", w[1]); fw.appendChild(n);
      if (old) { old.classList.remove("on"); old.classList.add("out"); setTimeout(() => old.remove(), 600); }
      fw.style.width = n.offsetWidth + "px";
      requestAnimationFrame(() => n.classList.add("on"));
    }

    const clock = (n) => `${Math.floor(n / 60)}:${String(n % 60).padStart(2, "0")}`;
    const shown = (id) => { const e = $(id); return !!e && !e.classList.contains("hidden"); };
    // Per session: which session was seen building, and how long each took.
    // A build that finished in another session must not "arrive" here with
    // that session's time (it played A's arrival and A's 0:20 on B).
    let mode = "", builtSecs = null, lastSecs = 0, arriveT = 0, wasBuilding = null, seenSid = null;
    const secsBy = new Map();

    // What the app may do, said plainly: what it does, then what it cannot.
    const DOES = {
      "ui.window": "opens a window", "store.kv": "keeps its own data", "store.sql": "keeps its own records",
      "store.shared": "shares its data by invite code", "store.group": "shares data with its maker's other apps",
      "net": "reaches the internet", "fs.read": "reads files you choose", "fs.write": "saves files you choose",
      "audio.capture": "uses the microphone", "audio.playback": "plays sound", "camera": "uses the camera",
      "speech": "turns your speech into text", "notify": "shows notifications", "clipboard.read": "reads what you copied",
      "clipboard.write": "copies things for you",
    };
    function permLine() {
      const st = app(), res = st && st.session && st.session.result;
      const caps = ((res && res.asks) || []).map(String);
      if (!res) return "";
      const does = [];
      caps.forEach((c) => {
        const k = Object.keys(DOES).find((x) => c === x || c.startsWith(x + ":") || c.startsWith(x + "."));
        if (k && !does.includes(DOES[k])) does.push(DOES[k]);
      });
      const nots = [];
      if (!caps.some((c) => c.startsWith("net."))) nots.push("internet");
      if (!caps.some((c) => c.startsWith("camera."))) nots.push("camera");
      if (!caps.some((c) => c.startsWith("fs."))) nots.push("other files");
      const list = (a) => a.length > 1 ? a.slice(0, -1).join(", ") + " and " + a[a.length - 1] : a[0] || "";
      const b = does.map((d) => `<b>${esc(d)}</b>`);
      let h = does.length ? list(b) + "." : "<b>Opens a window</b> and asks for nothing else.";
      h = h.replace(/^<b>(.)/, (m, c) => "<b>" + c.toUpperCase());
      if (nots.length) h += ` No ${esc(nots.length > 1 ? nots.slice(0, -1).join(", ") + " or " + nots[nots.length - 1] : nots[0])}.`;
      return `${ico("lock", "kr-ico")}<span>${h} <button type="button" data-fa="details">Details</button></span>`;
    }
    function doneRow() {
      const nm = (($("doneName") || {}).textContent || "").trim();
      q(".kr-dn b", f).textContent = nm || "your-app.krate";
      const ver = ((q(".kr-ver") || {}).textContent || "").trim();
      const bits = [];
      if (/^v\d+$/.test(ver)) bits.push(`<span class="kr-mono">${esc(ver)}</span>`);
      const size = (($("doneSize") || {}).textContent || "").trim();
      if (size) bits.push(`<span>${esc(size)}</span>`);
      bits.push("<span>wasm32</span>");
      if (builtSecs != null) bits.push(`<span>built in ${clock(builtSecs)}</span>`);
      const sm = q(".kr-dn small", f), h = bits.join("");
      if (sm.innerHTML !== h) sm.innerHTML = h;
      const p = q(".kr-perm", f), ph = permLine();
      if (p.dataset.h !== ph) { p.dataset.h = ph; p.innerHTML = ph; }
      const v = $("doneVerdict"), vp = q(".kr-fverd p", f);
      const vt = v && !v.classList.contains("hidden") ? v.textContent.trim() : "";
      if (vp.textContent !== vt) vp.textContent = vt;
      f.classList.toggle("kr-hasverd", !!vt);
      const shot = $("shot"), src = shot && !shot.classList.contains("hidden") ? shot.getAttribute("src") || "" : "";
      if (src && img.getAttribute("src") !== src) img.setAttribute("src", src);
      if (!src) img.removeAttribute("src");
      f.classList.toggle("kr-noshot", !src);
    }
    function failBlock() {
      const t = (($("failTitle") || {}).textContent || "").trim();
      const why = (($("failWhy") || {}).textContent || "").trim();
      const stopped = /stop/i.test(t);
      q(".kr-ffail h5 span", f).textContent = t;
      q(".kr-ffail p", f).textContent = why;
      q(".kr-ffail p", f).hidden = !why;
      const acts = q(".kr-facts2", f);
      // Report an issue rides along: it lived only on the old failure pane,
      // which this card hides, so a failed build could not be reported.
      const want = [["retryBtn", "kr-dark"], ["switchAiBtn", "kr-ghost"], ["makeitBtn", "kr-plain", "Send this to Krate"], ["reportBtn", "kr-plain", "Report an issue"]]
        .filter(([id]) => { const b = $(id); return b && !b.classList.contains("hidden") && !b.hidden; });
      const key = want.map(([id]) => id + ":" + ($(id).textContent || "")).join("|") + "|" + stopped;
      if (acts.dataset.k !== key) {
        acts.dataset.k = key; acts.innerHTML = "";
        want.forEach(([id, cls, label], i) => {
          if (stopped && i > 0) return;
          const b = document.createElement("button"); b.type = "button"; b.className = "btn " + cls;
          b.textContent = label || $(id).textContent.trim();
          b.addEventListener("click", () => press($(id)));
          acts.appendChild(b);
        });
        if (($("buildLog") || {}).textContent) {
          const l = document.createElement("button"); l.type = "button"; l.className = "btn kr-plain"; l.dataset.fa = "log";
          l.innerHTML = `${ico("term", "kr-ico")}<span>Show details</span>`; acts.appendChild(l);
        }
      }
      return stopped;
    }
    function paintLogBtn() {
      qa('[data-fa="log"] span', f).forEach((s) => { s.textContent = logOn ? "Hide details" : "Show details"; });
      if (f.classList.contains("kr-logon") !== logOn) { f.classList.toggle("kr-logon", logOn); if (logOn) { lgKey = ""; paintLog(); } }
    }

    function tick() {
      const st = app();
      const s = shown("stateDone") ? "done" : shown("stateFailed") ? "failed" : shown("stateBuilding") ? "building" : shown("statePlanning") ? "planning" : "idle";
      let next = s;
      const sid = st && st.session ? st.session.id : null;
      if (sid !== seenSid) { seenSid = sid; arriveT = 0; wasBuilding = null; builtSecs = secsBy.has(sid) ? secsBy.get(sid) : null; }
      // Arriving: the app shows up small in the inset, then settles -- only
      // in the session whose build was watched.
      if (s === "done" && wasBuilding && wasBuilding === sid) { wasBuilding = null; builtSecs = secsBy.has(sid) ? secsBy.get(sid) : lastSecs; arriveT = Date.now(); }
      if (s === "done" && arriveT && Date.now() - arriveT < 1500) next = "arrive";
      else if (s === "done") arriveT = 0;
      if (s === "building") wasBuilding = sid;
      else if (s !== "done") wasBuilding = null;
      if (s === "building" && mode !== "building" && mode !== "arrive") builtSecs = null;
      if (s === "failed" && failBlock()) next = "stopped";
      if (s !== "planning") f.classList.remove("kr-busy");
      if (mode !== next) {
        mode = next; f.dataset.st = next;
        if (next !== "building") f.classList.remove("kr-long");
      }
      paintLogBtn();
      if (s === "building") {
        const r = runs.get(liveKey());
        const last = r && r.rows.length ? r.rows[r.rows.length - 1].id : "";
        const ph = /^w:/.test(last) ? "write" : last === "read" ? "plan" : last;
        f.dataset.ph = ph;
        glyph(GLYPH[ph] || "think");
        sentence(ph);
        const secs = st && st.startedAt ? Math.max(0, Math.floor((Date.now() - st.startedAt) / 1000)) : 0;
        lastSecs = secs; fel.textContent = clock(secs);
        if (sid) secsBy.set(sid, secs);
        f.classList.toggle("kr-long", secs >= 45);
        return;
      }
      f.dataset.ph = "";
      glyph("think");
      if (next === "arrive") { doneRow(); plain(`Ready · in ${clock(builtSecs || 0)}`, "kr-fok"); return; }
      if (s === "done") { doneRow(); fel.textContent = ""; return; }
      if (s === "failed") { plain(next === "stopped" ? "Stopped" : /open|run|start/i.test(q(".kr-ffail h5 span", f).textContent) ? "Did not open" : "Did not finish", next === "stopped" ? "" : "kr-fbad"); fel.textContent = builtSecs == null && lastSecs ? clock(lastSecs) : ""; return; }
      fel.textContent = "";
      if (s === "planning") {
        const waiting = shown("planAsk") ? "Waiting for your answer" : shown("planActions") ? "Waiting for your go" : "";
        const title = (($("planTitle") || {}).textContent || "").trim();
        f.classList.toggle("kr-busy", !waiting);
        plain(waiting || (/checking your ai/i.test(title) ? "Checking your AI" : "Reading what you asked for"));
        return;
      }
      f.classList.remove("kr-busy");
      plain((($("idleNote") || {}).textContent || "Your app will appear here.").trim().replace(/\.$/, ""));
    }
    setInterval(tick, 250);
    ["statePlanning", "stateBuilding", "stateDone", "stateFailed", "stateIdle"].forEach((id) => watch($(id), { attributes: true, attributeFilter: ["class"] }, tick));
    tick();

    f.addEventListener("click", (e) => {
      const b = e.target.closest("[data-fa]"); if (!b) return;
      const k = b.dataset.fa;
      if (k === "run") return press($("openBtn"));
      if (k === "share") return press($("barShare"));
      if (k === "stop") return press($("stopBtn"));
      if (k === "details") { const t = q('#panelTabs [data-pane="details"]'); return t && t.click(); }
      if (k === "log") { logOn = !logOn; try { localStorage.setItem("kr-forge-details", logOn ? "1" : "0"); } catch (err) {} paintLogBtn(); return; }
      if (k === "more") {
        let a = null; try { a = currentApp(); } catch (err) {}
        const items = [{ icon: desktopApp() ? "folder" : "down", label: desktopApp() ? "Show in folder" : "Download the file", run: () => press($("filesSave")) }];
        if (a && desktopApp() && window.krIdeOpen) items.push({ icon: "code", label: "Open in the IDE", run: async () => {
          let dir = ""; try { dir = await sourceDirOf(a); } catch (err) {}
          if (dir) window.krIdeOpen(dir, (a.name || "").replace(/\.krate$/, "") || undefined); else toast("This app's source is not on this computer");
        } });
        items.push({ icon: "file", label: "View the code", run: () => { const t = q('#panelTabs [data-pane="code"]'); if (t) t.click(); } });
        const src = $("sourceBtn");
        if (src && !src.classList.contains("hidden")) items.push({ icon: "folder", label: "Open the source project", run: () => press(src) });
        openPop(b, items, { above: true, right: true });
      }
    });
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
    // One download per press: presses while it fills do nothing, and the
    // button says what happened, then goes back to Get (it said
    // "Downloading" for ever and downloaded again on every press).
    if (btn && (btn.classList.contains("ing") || btn.classList.contains("got"))) return;
    if (btn) {
      const t = q(".gt", btn), was = t ? t.textContent : "";
      btn.classList.add("ing");
      setTimeout(() => {
        btn.classList.remove("ing"); btn.classList.add("got");
        if (t) t.textContent = desktopApp() ? "Opened in your browser" : "Downloaded";
        setTimeout(() => { btn.classList.remove("got"); if (t) t.textContent = was; }, 2600);
      }, 1300);
    }
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
    // The hub says which categories have apps; an older hub does not, and
    // then they come from the apps themselves (every pill showed, empty ones
    // too). "More" (apps) stays: it holds whatever fits nowhere else.
    const fromApps = Array.isArray(st.cloud) && st.cloud.length ? [...new Set(st.cloud.flatMap((a) => a.cats || (a.meta && a.meta.category ? [a.meta.category] : [])))].concat(["apps"]) : null;
    const present = st.cloudCats || fromApps;
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
    // app.js's note lives in this page while it shows, and goes home before
    // the page is redrawn: drawn over, it was destroyed, and the third app
    // opened in one visit showed the old app while Run it ran the new one.
    const noteEl = $("detailNote"), noteHome = noteEl ? noteEl.parentElement : null;
    const NOT = [[/^net\./, "No network"], [/^camera\./, "No camera"], [/^(fs\.|ui\.dialog:file|ui\.dialog:open)/, "None of your files"]];
    function caps(list) {
      const box = q(".kd-asks", kd); if (!box) return;
      let words = (c) => c; try { words = capWords; } catch (e) {}
      // What every app may do without asking (print, the clock, the
      // language, random numbers) is not an ask: twelve lines of it buried
      // the two that matter.
      const plumbing = /^(io\.|time\.|locale\.|random\.)/;
      const yes = (list || []).filter((c) => !plumbing.test(String(c))).map((c) => { try { return capWords(c); } catch (e) { return c; } }).filter((w, i, a) => w && a.indexOf(w) === i);
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
      if (noteEl && noteHome && kd.contains(noteEl)) noteHome.appendChild(noteEl);
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
    window.krGalCapsFailed = (why) => { const box = q(".kd-asks", kd); if (box) box.innerHTML = `<span class="kd-dim">${esc(why || "Could not read what it asks for just now. Krate still checks it when the app opens.")}</span>`; };
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
      let opening = false;
      window.openApp = async function (which, version) {
        // One open per press: a double-click launched the app twice.
        if (opening) return { kind: "busy" };
        opening = true;
        let a = null; try { a = (which && which.path) ? which : currentApp(); } catch (e) {}
        const name = a ? (a.name || "").replace(/\.krate$/, "").replace(/[-_]+/g, " ").replace(/\b\w/g, (c) => c.toUpperCase()) : "your app";
        const n = pill("krOpen"); n.classList.remove("off", "lift"); n.classList.add("on", "opening");
        q(".kr-swp", n).innerHTML = `<span><b>Opening ${esc(name)}</b></span>`;
        setTimeout(() => n.classList.add("lift"), 380);
        const t0 = performance.now();
        let r = { kind: "failed", why: "" };
        try { r = (await oa(which, version)) || { kind: "opened" }; } catch (e) { r = { kind: "failed", why: String(e) }; } finally {
          opening = false;
          const secs = ((performance.now() - t0) / 1000).toFixed(1);
          const why = (r.why || "").split(/(?<=\.)\s/)[0].slice(0, 90);
          // The pill says what really happened: a tab that is asking says
          // nothing (its sheet is talking), a download says so.
          if (r.kind === "asking") n.classList.remove("on", "opening", "lift");
          else window.krSwap(q(".kr-swp", n),
            r.kind === "failed" ? `<b>${esc(name)} did not open</b><small>${esc(why || "Try again in a moment.")}</small>`
            : r.kind === "downloaded" ? `<b>${esc(name)} downloaded</b><small>Double-click the file to open it.</small>`
            : `<b>${esc(name)}</b><small>${+secs >= 0.1 ? `opened in ${secs} s` : "opened"}</small>`);
          setTimeout(() => n.classList.remove("on", "opening", "lift"), r.kind === "failed" ? 4200 : 2600);
        }
        return r;
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
    wait.innerHTML = `${window.krIso ? window.krIso(30, "breathe") : ""}<span><b>Finish in your browser</b><small>This page moves on by itself once you have signed in.</small>` +
      `<span class="kr-gw-acts"><button type="button" class="kr-gw-a" data-gw="again" hidden>Open the browser again</button><button type="button" class="kr-gw-a" data-gw="code" hidden>Use a code instead</button><button type="button" class="kr-gw-a" data-gw="cancel">Cancel</button></span></span>`;
    const start = $("gateStart"); if (start) start.after(wait);
    const btn = $("loginBrowserBtn");
    // Never a wait with no way out (K-971): Cancel at any time, and after a
    // minute the browser can be opened again (the earlier page still works,
    // Studio keeps several sign-ins waiting) or a code used instead.
    let slow = 0;
    const showWait = (on) => {
      wait.hidden = !on; clearTimeout(slow);
      qa("[data-gw=again], [data-gw=code]", wait).forEach((b) => { b.hidden = true; });
      if (on) slow = setTimeout(() => { qa("[data-gw=again], [data-gw=code]", wait).forEach((b) => { b.hidden = false; }); q("small", wait).textContent = "Taking a while? The page may be in another browser window."; }, 60000);
      else q("small", wait).textContent = "This page moves on by itself once you have signed in.";
    };
    if (btn) btn.addEventListener("click", () => showWait(true), true);
    wait.addEventListener("click", (e) => {
      const b = e.target.closest("[data-gw]"); if (!b) return;
      if (b.dataset.gw === "cancel") { showWait(false); return; }
      if (b.dataset.gw === "again") { press(btn); showWait(true); return; }
      if (b.dataset.gw === "code") {
        const c = $("loginBtn");
        // The code button is switched off while the engine did not answer at
        // boot; this asks anyway (it may be back) and says why if not,
        // instead of a press that did nothing.
        if (c && c.disabled) { showWait(false); try { login(); } catch (e) {} return; }
        showWait(false); if (c) { c.classList.remove("hidden"); press(c); }
      }
    });
    // The code screen gets a way back too: it hid Skip and the buttons and
    // waited up to fifteen minutes.
    const code = $("gateCode");
    if (code && !q(".kr-gw-back", code)) {
      const back = document.createElement("button"); back.type = "button"; back.className = "kr-gw-a kr-gw-back"; back.textContent = "Cancel";
      back.addEventListener("click", () => { code.classList.add("hidden"); if (start) start.classList.remove("hidden"); });
      code.appendChild(back);
    }
    const err = $("gateError");
    if (err) watch(err, { attributes: true, attributeFilter: ["class"] }, () => { if (!err.classList.contains("hidden")) showWait(false); });
    const sync = () => {
      const on = !g.classList.contains("hidden");
      document.body.classList.toggle("kr-gating", on);
      if (on && window.krStack) { qa(".kr-gmark .ly", g).forEach((l) => l.classList.remove("on")); window.krStack(g, 200); }
      if (!on) showWait(false);
    };
    watch(g, { attributes: true, attributeFilter: ["class"] }, sync); sync();
  })();

  /* ---- signing in: one small popup ---------------------------------------- */
  // The design's sign-in (krate-signin.html): the mark stacks in over a plain
  // question; GitHub first, then Google, then an email link, and a code for a
  // browser somewhere else. Every door ends in the same "you're signed in".
  // The popup keeps its place and only changes height while its inside
  // crossfades. It drives the real sign-in: app.js's beginBrowserSignIn and
  // watcher, `login_email`, the device-code flow, and the login-step events,
  // which app.js sends here while the popup is the surface asking.
  (function signInPopup() {
    const GH = '<svg viewBox="0 0 16 16" fill="currentColor" aria-hidden="true"><path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82a7.42 7.42 0 0 1 4 0c1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.01 8.01 0 0 0 16 8c0-4.42-3.58-8-8-8Z"/></svg>';
    const GG = '<svg viewBox="0 0 18 18" aria-hidden="true"><path fill="#4285F4" d="M17.64 9.2c0-.64-.06-1.25-.16-1.84H9v3.48h4.84a4.14 4.14 0 0 1-1.8 2.72v2.26h2.92c1.7-1.57 2.68-3.88 2.68-6.62z"/><path fill="#34A853" d="M9 18c2.43 0 4.47-.8 5.96-2.18l-2.92-2.26c-.8.54-1.84.86-3.04.86-2.34 0-4.32-1.58-5.03-3.7H.96v2.33A9 9 0 0 0 9 18z"/><path fill="#FBBC05" d="M3.97 10.72a5.4 5.4 0 0 1 0-3.44V4.95H.96a9 9 0 0 0 0 8.1l3.01-2.33z"/><path fill="#EA4335" d="M9 3.58c1.32 0 2.5.45 3.44 1.35l2.58-2.58A9 9 0 0 0 .96 4.95l3.01 2.33C4.68 5.16 6.66 3.58 9 3.58z"/></svg>';
    const ENV = '<div class="kr-env"><svg width="124" height="96" viewBox="0 0 124 96"><path class="back" d="M6 30 L62 2 L118 30 V88 H6 Z"/></svg><div class="lk">Sign in to Krate<i></i><i></i></div><svg width="124" height="96" viewBox="0 0 124 96"><path class="front" d="M6 30 L62 64 L118 30 V84 a6 6 0 0 1 -6 6 H12 a6 6 0 0 1 -6 -6 Z"/><path class="front2" d="M6 30 L62 64 L118 30"/></svg></div>';
    const PROV = { github: ["GitHub", GH], google: ["Google", GG] };
    const iso = (n, c) => (window.krIso ? window.krIso(n, c) : "");
    const web = () => !desktopApp();
    let LAST = ""; try { LAST = localStorage.getItem("kr-signin-last") || ""; } catch (e) {}
    const wrapEl = document.createElement("div");
    wrapEl.className = "kr-lgw"; wrapEl.id = "krLogin"; wrapEl.hidden = true;
    wrapEl.setAttribute("role", "dialog"); wrapEl.setAttribute("aria-modal", "true"); wrapEl.setAttribute("aria-labelledby", "krLgH");
    wrapEl.innerHTML = `<div class="kr-lgc" tabindex="-1"><button type="button" class="kr-x kr-lg-x" aria-label="Close">${ico("x")}</button><div class="kr-lg-wrap"><div class="kr-lg-in"></div></div></div>`;
    document.body.appendChild(wrapEl);
    const box = q(".kr-lg-in", wrapEl), wr = q(".kr-lg-wrap", wrapEl);
    let opts = {}, timers = [], tick = 0, stateNow = "";
    const later = (f, ms) => timers.push(setTimeout(f, ms));
    const clear = () => { timers.forEach(clearTimeout); timers = []; clearInterval(tick); };
    const subFor = () => opts.why || (web() ? "One account for everything you make and share. Making an app here needs one." : "One account for everything you make and share. You only need it to publish.");
    const V = {
      start: () => `<div class="kr-lg-hd"><span class="kr-lg-mk">${iso(42, "stack")}</span><h3 id="krLgH">Sign in to Krate</h3><p>${esc(subFor())}</p></div>
        <div class="kr-lg-btns"><button type="button" class="kr-lg-b dark" data-lg="github">${GH}Continue with GitHub${LAST === "github" ? '<span class="last">Last used</span>' : ""}</button><button type="button" class="kr-lg-b" data-lg="google">${GG}Continue with Google${LAST === "google" ? '<span class="last">Last used</span>' : ""}</button></div>
        <div class="kr-lg-or">or</div>
        <div><form class="kr-lg-mail" novalidate><input type="email" placeholder="Continue with email" autocomplete="email" aria-label="Your email"><button type="submit" aria-label="Email me a link">${ico("arrow")}</button></form><div class="kr-lg-msg">That doesn't look like an email yet.</div></div>
        <p class="kr-lg-fine">By continuing you agree to the <a href="https://krate.tech/terms/" target="_blank" rel="noopener">terms</a> and the <a href="https://krate.tech/privacy/" target="_blank" rel="noopener">privacy page</a>.</p>
        <div class="kr-lg-ft">${ico("lock")}<span>Krate never sees your password</span><span class="kr-grow"></span>${web() ? "" : '<button type="button" data-lg="code">Use a code</button>'}</div>`,
      browser: (o) => { const [n, logo] = PROV[o.via];
        return `<div class="kr-lg-hd"><div class="kr-lg-link"><span class="t">${iso(24)}</span><span class="dots"><i></i><i></i><i></i><i></i></span><span class="t">${logo}</span></div><h3 id="krLgH">Finish in your browser</h3><p>${n} opened in your browser. Say yes there, and this moves on by itself.</p></div>
        <div class="kr-lg-wait"><span class="kr-lg-spin"></span>Waiting for ${n}</div>
        <div class="kr-lg-row"><button type="button" class="btn kr-ghost" data-lg="back">${ico("left")}Back</button><button type="button" class="btn kr-ghost" data-lg="reopen" data-via="${o.via}">${ico("ext")}Open it again</button></div><div class="kr-lg-pad"></div>`; },
      email: (o) => `<div class="kr-lg-hd">${ENV}<h3 id="krLgH">Check your email</h3><p>We sent a link to <b>${esc(o.email)}</b>. Open it on this computer. It works for 15 minutes.</p></div>
        <div class="kr-lg-wait"><span class="kr-lg-spin"></span>Waiting for you to open it</div>
        <div class="kr-lg-row"><button type="button" class="kr-lnk" data-lg="back">Use another email</button><span>·</span><button type="button" class="kr-lnk" data-lg="resend" disabled>Send again in <span class="cnt">0:30</span></button></div><div class="kr-lg-pad"></div>`,
      code: (o) => { const c = String(o.code || "").replace(/[^A-Za-z0-9]/g, "");
        return `<div class="kr-lg-hd"><span class="kr-lg-mk">${iso(42)}</span><h3 id="krLgH">Sign in with a code</h3><p>${o.code ? `On any phone or computer, go to <b>${esc(String(o.url || "github.com/login/device").replace(/^https?:\/\//, ""))}</b> and type this in.` : "Getting a code from GitHub…"}</p></div>
        <div class="kr-lg-code">${o.code ? [...c].map((ch, i) => (i === Math.floor(c.length / 2) ? '<span class="dash"></span>' : "") + `<span class="ch" style="--i:${i}">${esc(ch)}</span>`).join("") + `<button type="button" class="cp" data-lg="copy" title="Copy the code">${ico("copy")}</button>` : '<span class="kr-lg-spin"></span>'}</div>
        <div class="kr-lg-wait"><span class="kr-lg-spin"></span><span>Waiting for the code</span></div>
        <div class="kr-lg-row"><button type="button" class="btn kr-ghost" data-lg="back">${ico("left")}Back</button>${o.url ? `<button type="button" class="btn kr-ghost" data-lg="codeurl">${ico("ext")}Open the page</button>` : ""}</div><div class="kr-lg-pad"></div>`; },
      done: (o) => { const a = o.account || {}, who = a.name || a.login || "you";
        return `<div class="kr-lg-hd"><span class="kr-lg-av">${esc(who.trim().charAt(0).toUpperCase() || "K")}<span class="ok"><svg viewBox="0 0 12 12"><path d="M2.5 6.2 L5 8.6 L9.6 3.6"/></svg></span></span><h3 id="krLgH">You're signed in</h3><p>${esc(a.login ? "@" + a.login : who)}${o.via && PROV[o.via] ? " · with " + PROV[o.via][0] : o.via === "email" ? " · with an email link" : ""}</p></div>
        <div class="kr-lg-done-bar"><i></i></div><div class="kr-lg-pad"></div>`; },
      error: (o) => `<div class="kr-lg-hd"><span class="kr-lg-bad">!</span><h3 id="krLgH">${esc(o.title || "That sign-in did not finish")}</h3><p>${esc(o.why || "Nothing changed. Try again, or pick another way.")}</p></div>
        ${o.email ? `<button type="button" class="kr-lg-b dark sm" data-lg="resend">Send a new link</button><button type="button" class="kr-lnk" data-lg="back">Use another way</button>` : '<button type="button" class="kr-lg-b dark sm" data-lg="back">Try again</button>'}<div class="kr-lg-pad"></div>`,
    };
    // Tab stays inside the popup while it is open: it walked out to the
    // page underneath, which the dialog covers.
    wrapEl.addEventListener("keydown", (e) => {
      if (e.key !== "Tab" || wrapEl.hidden) return;
      const items = [...wrapEl.querySelectorAll('button, a[href], input, [tabindex]:not([tabindex="-1"])')].filter((x) => !x.disabled && x.offsetParent !== null);
      if (!items.length) return;
      const first = items[0], last = items[items.length - 1];
      if (e.shiftKey && (document.activeElement === first || !wrapEl.contains(document.activeElement))) { e.preventDefault(); last.focus(); }
      else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first.focus(); }
    });
    // Studio stops looking after ten minutes; the wait line stops saying it
    // moves on by itself, and offers to look again.
    window.krLoginStale = () => {
      if (wrapEl.hidden || !(stateNow === "browser" || stateNow === "email")) return;
      const p = q(".kr-lg-wait", box);
      if (p) p.innerHTML = `<span class="kr-lg-spin"></span>Still looking, every few seconds. Signed in already? <button type="button" class="kr-lnk" data-lg="look">Check now</button>`;
    };
    function go(st, o = {}) {
      clear();
      const live = !wrapEl.hidden, h0 = wr.offsetHeight;
      stateNow = st; box.dataset.st = st; box.innerHTML = V[st](o); box._o = o;
      if (live && h0) {
        const h1 = box.offsetHeight; wr.style.height = h0 + "px"; void wr.offsetHeight; wr.style.height = h1 + "px";
        later(() => { wr.style.height = ""; }, 480);
        box.classList.remove("kr-lg-sw"); void box.offsetWidth; box.classList.add("kr-lg-sw");
      }
      if (st === "start" && window.krStack) window.krStack(wrapEl, live ? 120 : 260);
      if (st === "email") {
        let n = 30;
        tick = setInterval(() => { n--; const b = q('[data-lg="resend"]', box); if (!b) { clearInterval(tick); return; } if (n <= 0) { clearInterval(tick); b.disabled = false; b.textContent = "Send it again"; return; } const c = q(".cnt", b); if (c) c.textContent = `0:${String(n).padStart(2, "0")}`; }, 1000);
        // An email link lasts fifteen minutes.
        // The address goes with it, so a new link is one press.
        const email = o.email;
        later(() => { if (stateNow === "email") go("error", { title: "That link has run out", why: "Links work for 15 minutes, and each one only once. Send yourself a new one.", email }); }, 15 * 60 * 1000);
      }
      if (st === "browser") later(() => { if (stateNow === "browser") { const p = q(".kr-lg-wait", box); if (p) p.lastChild.textContent = " Still waiting. The page may be in another browser window."; } }, 60000);
      if (st === "done") later(() => { close(true); }, 2000);
      if (live && st !== "start") setTimeout(() => { const c = q(".kr-lgc", wrapEl); if (c) c.focus({ preventScroll: true }); }, 60);
    }
    function surface(on) { try { state.loginSurface = on ? "popup" : "gate"; } catch (e) {} }
    function open(o = {}) {
      opts = o; closePop();
      if (!web()) surface(true);
      document.body.classList.add("kr-lgopen");
      wr.style.height = ""; wrapEl.hidden = false; wrapEl.classList.remove("out");
      go("start");
      // Focus inside the dialog for the keyboard, without a ring on a button
      // nobody has chosen yet.
      setTimeout(() => { const c = q(".kr-lgc", wrapEl); if (c) c.focus({ preventScroll: true }); }, 350);
    }
    function close(signedIn) {
      if (wrapEl.hidden) return;
      clear(); wrapEl.classList.add("out");
      if (!web()) surface(false);
      document.body.classList.remove("kr-lgopen");
      setTimeout(() => { wrapEl.hidden = true; wrapEl.classList.remove("out"); }, reduce ? 0 : 260);
      const done = opts.onDone, cancel = opts.onClose, acct = box._o && box._o.account;
      opts = {};
      if (signedIn) { if (acct) toast(`Signed in as ${acct.login || acct.name || "you"}`); if (done) done(acct); }
      else if (cancel) cancel();
    }
    // The real sign-in, by door.
    function startProvider(via) {
      try { localStorage.setItem("kr-signin-last", via); LAST = via; } catch (e) {}
      if (web()) {
        // In a tab the sign-in page goes straight on to the provider and
        // comes back to the Studio, where the kept request carries on.
        try { localStorage.setItem("krate_next", "studio"); } catch (e) {}
        location.href = "/login/?next=studio#" + via;
        return;
      }
      go("browser", { via });
      try { beginBrowserSignIn(via).catch((err) => go("error", { why: String(err && err.message || err) })); } catch (e) { go("error", {}); }
    }
    async function startEmail(email) {
      if (web()) {
        let nonce = ""; try { nonce = window.KrateSignIn ? window.KrateSignIn.begin() : ""; } catch (e) {}
        try { localStorage.setItem("krate_next", "studio"); } catch (e) {}
        const r = await fetch("https://hub.krate.tech/login/email", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ email, from: "web", nonce }) }).catch(() => null);
        if (!r) throw new Error("Krate could not be reached. Check your connection.");
        if (!r.ok) throw new Error(await r.text().catch(() => "The email could not be sent."));
        return;
      }
      await invoke("login_email", { email });
      try { watchSignIn(); } catch (e) {}
    }
    wrapEl.addEventListener("click", async (e) => {
      if (e.target === wrapEl) { close(false); return; }
      if (e.target.closest(".kr-lg-x")) { close(false); return; }
      const b = e.target.closest("[data-lg]"); if (!b) return;
      const a = b.dataset.lg, o = box._o || {};
      if (a === "github" || a === "google") startProvider(a);
      else if (a === "reopen") { try { beginBrowserSignIn(b.dataset.via).catch(() => {}); toast(`Opened ${PROV[b.dataset.via][0]} again`); } catch (err) {} }
      else if (a === "back") go("start");
      else if (a === "look") { try { watchSignIn(); const r = await invoke("account_status"); if (r && r.signed_in) signedInFromBrowser(r); else { const p = q(".kr-lg-wait", box); if (p) p.innerHTML = '<span class="kr-lg-spin"></span>Not yet. Waiting again'; } } catch (err) {} }
      else if (a === "code") { go("code", {}); try { invoke("account_login").catch((err) => { if (stateNow === "code") go("error", { why: String(err && err.message || err) }); }); } catch (err) {} }
      else if (a === "copy") { try { await navigator.clipboard.writeText(o.code || ""); toast("Code copied"); } catch (err) {} }
      else if (a === "codeurl") { try { invoke("open_external", { url: o.url }).catch(() => {}); } catch (err) {} }
      else if (a === "resend") { b.disabled = true; try { await startEmail(o.email); go("email", o); } catch (err) { go("error", { why: String(err.message || err) }); } }
    });
    wrapEl.addEventListener("input", (e) => {
      const f = e.target.closest(".kr-lg-mail"); if (!f) return;
      f.classList.toggle("ok", /^[^\s@]+@[^\s@]+\.[^\s@]{2,}$/.test(e.target.value.trim())); f.classList.remove("bad");
    });
    wrapEl.addEventListener("submit", async (e) => {
      e.preventDefault();
      const f = e.target.closest(".kr-lg-mail"); if (!f) return;
      const v = q("input", f).value.trim();
      if (!f.classList.contains("ok")) { f.classList.remove("bad"); void f.offsetWidth; f.classList.add("bad"); q("input", f).focus(); return; }
      const btn = q("button", f); btn.disabled = true;
      try { await startEmail(v); try { localStorage.setItem("kr-signin-last", "email"); } catch (err) {} go("email", { email: v }); }
      catch (err) { btn.disabled = false; const m = q(".kr-lg-msg", box); if (m) m.textContent = String(err.message || err); f.classList.add("bad"); }
    });
    document.addEventListener("keydown", (e) => { if (e.key === "Escape" && !wrapEl.hidden) { e.stopPropagation(); close(false); } }, true);
    // app.js hands the popup its events while it is the surface asking.
    window.krLoginStep = (step) => {
      if (step.step === "code") go("code", { code: step.code, url: step.url });
      else if (step.step === "done") window.krLoginDone({ signed_in: true, login: step.login, name: step.name });
      else if (step.step === "adopted") { invoke("account_status").then((a) => { if (a && a.signed_in) { try { state.account = a; renderAccount(); } catch (e) {} window.krLoginDone(a); } }).catch(() => {}); }
      else if (step.step === "handoff-failed") go("error", { title: "That sign-in did not reach Krate", why: "The browser page from before will not work now. Try again; it only takes a moment." });
      else if (step.step === "error") go("error", { why: String(step.why || "") });
    };
    window.krLoginDone = (a, via) => {
      if (wrapEl.hidden || stateNow === "done") return;
      try { clearInterval(state.signInWatch); state.signInSince = 0; } catch (e) {}
      go("done", { account: a, via: via || (box._o && box._o.via) || (stateNow === "email" ? "email" : "") });
    };
    // The web: someone signed in from an email link in another tab.
    window.addEventListener("storage", (e) => { if (e.key === "krate_tok" && e.newValue && !wrapEl.hidden && web()) { go("done", { account: {}, via: "email" }); later(() => location.reload(), 1600); } });
    window.krSignIn = open;

    // Where it opens. The sign-in page (after a sign-out, or Settings' Sign
    // in) is the popup over an empty page: closing it is "skip, I just want
    // to build". Not when the page is showing an error of its own.
    const gate = $("viewGate");
    if (gate && desktopApp()) {
      watch(gate, { attributes: true, attributeFilter: ["class"] }, () => {
        const on = !gate.classList.contains("hidden");
        const err = $("gateError");
        if (on && wrapEl.hidden && !(err && !err.classList.contains("hidden") && /engine|could not/i.test(err.textContent))) {
          open({ onDone: () => { try { refreshAccountAndEnter(); } catch (e) {} }, onClose: () => { if (!gate.classList.contains("hidden")) press($("gateSkip")); } });
        } else if (!on && !wrapEl.hidden && stateNow !== "done") close(false);
      });
    }
    // Publishing: app.js asks for an account inside the publish sheet; the
    // popup asks instead, and the publish carries on when it lands.
    const ps = $("pubSignin");
    if (ps && desktopApp()) {
      watch(ps, { attributes: true, attributeFilter: ["class"] }, () => {
        if (ps.classList.contains("hidden") || !wrapEl.hidden) return;
        // app.js's own reason when it has one ("Your sign-in expired…").
        const said = (($("pubSigninWhy") || {}).textContent || "").trim();
        const why = /expired|again/i.test(said) ? said : "Sign in to publish. Your app stays on this computer until you do.";
        const back = () => { ps.classList.add("hidden"); const f = $("pubForm"); if (f) f.classList.remove("hidden"); try { state.loginSurface = "gate"; } catch (e) {} };
        open({
          why,
          // Finished where it started: the share pane publishes again with
          // its ring; the publish sheet publishes from its form.
          onDone: () => { const again = window.__krPubAgain; if (again) { window.__krPubAgain = null; back(); again(); return; } try { pubSigninDone(); } catch (e) {} },
          // Closed: back to the form, never left on a sign-in step with no
          // way out (K-995).
          onClose: () => { const fromPane = !!window.__krPubAgain; window.__krPubAgain = null; back(); if (fromPane) { const sh = $("publishSheet"); if (sh) sh.classList.add("hidden"); } },
        });
      });
    }
  })();

  /* ---- typed during a build: a small menu at the send button -------------- */
  // app.js asks with a sheet (stop and use this, or wait and do it after);
  // the design asks where the person just pressed, like the AI picker. The
  // rows press the sheet's own buttons, so what happens is app.js's.
  (function midMenu() {
    const sheet = $("midSheet"), send = $("send");
    if (!sheet || !send) return;
    watch(sheet, { attributes: true, attributeFilter: ["class"] }, () => {
      if (sheet.classList.contains("hidden") || sheet.dataset.krMenu) return;
      sheet.classList.add("hidden");
      const words = (($("midSub") || {}).textContent || "").trim();
      openPop(send, [
        { head: `<span><b>Still building</b><small>${esc(words)}</small></span>` },
        { icon: "stop", label: "Stop it and use this instead", sub: "Starts again with your new words", run: () => press($("midStopBtn")) },
        { icon: "right", label: "Wait, then do this", sub: "Runs right after, as a change", run: () => press($("midWaitBtn")) },
        "sep",
        { icon: "x", label: "Never mind", sub: "Your words stay in the box", run: () => { const box = $("prompt"); if (box) box.focus(); } },
      ], { big: true, above: true, right: true });
      // The menu is about the build that is running: when that build ends,
      // the question has gone and so does the menu.
      const mine = popEl, st = app(), was = st && st.buildingSession;
      const t = setInterval(() => {
        const now = app();
        if (popEl !== mine) { clearInterval(t); return; }
        if (!now || !now.buildingSession || now.buildingSession !== was || now.buildSettled) { clearInterval(t); closePop(); }
      }, 300);
    });
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

/* The conversation keeps its newest words in view. Cards are dressed and
 * grow after they are appended (a question's chips and answer box, a
 * plan's buttons), so a scroll at append time stopped short: on a phone the
 * question's Send and Skip sat under the box. While the person is at the
 * bottom it follows the bottom; once they scroll up to read, it lets them. */
(function stickThread() {
  const th = document.getElementById("thread");
  if (!th || !window.MutationObserver) return;
  let stick = true;
  th.addEventListener("scroll", () => { stick = th.scrollTop + th.clientHeight >= th.scrollHeight - 48; }, { passive: true });
  const follow = () => { if (stick) th.scrollTop = th.scrollHeight; };
  new MutationObserver(() => requestAnimationFrame(follow)).observe(th, { childList: true, subtree: true });
  if (window.ResizeObserver) {
    const ro = new ResizeObserver(() => follow());
    const watchLast = () => { const l = th.lastElementChild; if (l && !l.dataset.krObs) { l.dataset.krObs = "1"; ro.observe(l); } };
    new MutationObserver(watchLast).observe(th, { childList: true });
    watchLast();
  }
  // A session opened or a new one started: begin at the bottom again.
  document.addEventListener("click", (e) => { if (e.target.closest && e.target.closest(".sess-row, #homeSend")) stick = true; }, true);
})();
