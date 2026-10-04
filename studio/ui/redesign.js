/* Krate Studio, the 2026-10 design: the few behaviours the new look adds.
 *
 * Loaded after app.js (and, on the web, after the bridge). Everything here
 * sits inside one function so it declares no top-level name: app.js and
 * bridge.js share one global scope in a browser tab, and a clash blanks the
 * page (scope-test.mjs). It never calls the backend itself; the Port doors
 * press the same #homePort link app.js already wires to the folder picker.
 */
(function () {
  "use strict";
  const $ = (id) => document.getElementById(id);
  const reduce = window.matchMedia && matchMedia("(prefers-reduced-motion: reduce)").matches;

  /* ---- the word in the Home title: building, then shipping ------------- */
  const rw = $("homeRw");
  if (rw) {
    const words = [...rw.querySelectorAll(".w")];
    let at = 0;
    // Measured only while Home is on screen: a hidden view measures 0, and a
    // 0px box would let the question mark sit on top of the word.
    const fit = () => {
      const on = words[at];
      const w = on ? on.offsetWidth : 0;
      if (w) rw.style.width = w + "px"; else rw.style.removeProperty("width");
    };
    // Each word is watched, so a late font (wider fallback first) refits it.
    if (window.ResizeObserver) { const ro = new ResizeObserver(fit); words.forEach((w) => ro.observe(w)); }
    const show = (i) => {
      words.forEach((w, k) => { w.classList.toggle("on", k === i); w.classList.toggle("out", k === at && k !== i); });
      at = i; fit();
    };
    (document.fonts && document.fonts.ready ? document.fonts.ready : Promise.resolve()).then(fit);
    addEventListener("resize", fit);
    if (!reduce && words.length > 1) {
      setInterval(() => {
        const home = $("viewHome");
        if (document.hidden || !home || home.classList.contains("hidden")) return;
        show((at + 1) % words.length);
      }, 2800);
    }
  }

  /* ---- Create / Port on the Home composer ------------------------------ */
  const tabs = $("homeTabs");
  const pane = $("homePortPane");
  const bar = tabs ? tabs.closest(".bigbar") : null;
  const portLink = $("homePort");
  const startPort = () => { if (portLink) portLink.click(); };
  function placeInd() {
    if (!tabs) return;
    const on = tabs.querySelector("button.on");
    const ind = tabs.querySelector(".tab-ind");
    if (!on || !ind) return;
    ind.style.left = on.offsetLeft + "px";
    ind.style.width = on.offsetWidth + "px";
  }
  function setMode(mode) {
    if (!tabs || !bar) return;
    tabs.querySelectorAll("button[data-mode]").forEach((b) => {
      const on = b.dataset.mode === mode;
      b.classList.toggle("on", on);
      b.setAttribute("aria-selected", on ? "true" : "false");
    });
    bar.dataset.mode = mode;
    if (pane) pane.hidden = mode !== "port";
    const kb = $("homeKb");
    if (kb) kb.hidden = mode === "port";
    placeInd();
    if (mode === "create") { const ta = $("homePrompt"); if (ta) ta.focus(); }
  }
  if (tabs) {
    tabs.addEventListener("click", (e) => {
      const b = e.target.closest("button[data-mode]");
      if (b) setMode(b.dataset.mode);
    });
    (document.fonts && document.fonts.ready ? document.fonts.ready : Promise.resolve()).then(placeInd);
    addEventListener("resize", placeInd);
    setMode("create");
  }
  const go = $("homePortGo");
  if (go) go.addEventListener("click", startPort);

  /* ---- the crate on the build card ---------------------------------------
   * The forming frame used to be a grey window skeleton. It is now the Krate
   * crate, put together as the build goes: three grey shells while the AI
   * reads, each turning solid as the code is written, lit once it is being
   * tested, pressed together while it is finished. It only READS the
   * build's own stage (state.stageIndex, which app.js keeps for the one
   * build that is running) and never changes it, so it cannot alter what
   * the build does or claims. The first real test frame still replaces the
   * whole ghost, crate and all, exactly as before. */
  let crateN = 0;
  function crateSvg() {
    const k = "c" + (++crateN); // ids must be unique: there are two ghosts
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
  // Bits of code drifting in while it is written: [x, y, width, delay, colour].
  const BITS = [[-108, -45, 10, 0, "accent"], [115, -30, 6, 0.25, "violet"], [-86, 38, 8, 0.5, "pink"], [101, 45, 12, 0.75, "accent"],
    [-43, -68, 6, 1, "green"], [65, -64, 9, 1.2, "violet"], [-122, 8, 7, 1.45, "accent"], [126, 4, 10, 0.1, "pink"],
    [-22, 71, 8, 0.6, "violet"], [29, 75, 6, 1.35, "accent"], [-68, -22, 12, 0.85, "accent"], [79, -8, 7, 1.6, "green"]];
  const bitsHtml = '<div class="fg-bits" aria-hidden="true">' + BITS.map(([x, y, w, d, c]) =>
    `<i style="--x:${x}px;--y:${y}px;--w:${w}px;--d:${d}s;--c:var(--${c})"></i>`).join("") + "</div>";
  const ghosts = [...document.querySelectorAll(".forming-ghost")];
  ghosts.forEach((g) => {
    g.insertAdjacentHTML("beforeend", bitsHtml + crateSvg());
    g.dataset.ph = "plan";
    const svg = g.querySelector(".fg-crate");
    svg.dataset.ph = "plan";
    svg.classList.add("draw");
  });
  const buildGhost = $("formingGhost");
  const buildCrate = buildGhost ? buildGhost.querySelector(".fg-crate") : null;
  const PH = ["plan", "write", "build", "pack"]; // read, write, test, done
  let seenStart = null, writeAt = 0, lastPh = "";
  function setPh(ph) {
    if (ph === lastPh) return;
    lastPh = ph;
    buildCrate.dataset.ph = ph; buildGhost.dataset.ph = ph;
    if (ph !== "plan" && ph !== "write") buildCrate.querySelectorAll(".sl").forEach((s) => s.classList.add("on"));
  }
  function crateTick() {
    if (!buildCrate) return;
    const view = $("stateBuilding");
    if (!view || view.classList.contains("hidden")) return;
    let idx = -1, started = null;
    try { idx = state.stageIndex; started = state.startedAt; } catch (e) { /* app.js not loaded */ }
    if (started !== seenStart) {
      // A new build: grey shells again, drawn in from the top.
      seenStart = started; lastPh = ""; writeAt = 0;
      buildCrate.querySelectorAll(".sl").forEach((s) => s.classList.remove("on"));
      buildCrate.classList.remove("draw"); void buildCrate.getBoundingClientRect(); buildCrate.classList.add("draw");
    }
    const ph = PH[Math.max(0, Math.min(3, idx | 0))] || "plan";
    setPh(ph);
    if (ph === "write") {
      // One shell turns solid at once, the next two as the writing goes on.
      if (!writeAt) writeAt = Date.now();
      const t = (Date.now() - writeAt) / 1000;
      buildCrate.querySelectorAll(".sl").forEach((s, i) => { if (t >= [0, 25, 60][i]) s.classList.add("on"); });
    }
  }
  if (buildCrate) { crateTick(); setInterval(crateTick, 400); }

  /* ---- the done card: what the app may NOT do, beside what it may -------
   * app.js writes the trust line into the caption strip, which the new card
   * no longer shows. Its "cannot" part becomes a grey capsule after the
   * green ones. The words are app.js's own, copied as text; nothing new is
   * claimed and nothing is read from the app. */
  const asks = $("asks"), capTrust = $("capTrust");
  function mirrorCannot() {
    if (!asks || !capTrust) return;
    const nots = capTrust.textContent.split(" · ").map((s) => s.trim()).filter((s) => /^cannot /.test(s));
    const have = [...asks.querySelectorAll("li.no")].map((li) => li.textContent);
    if (have.length === nots.length && have.every((t, i) => t === nots[i])) return;
    asks.querySelectorAll("li.no").forEach((li) => li.remove());
    nots.forEach((t) => { const li = document.createElement("li"); li.className = "no"; li.textContent = t; asks.appendChild(li); });
  }
  if (asks && capTrust && window.MutationObserver) {
    const mo = new MutationObserver(mirrorCannot);
    mo.observe(asks, { childList: true });
    mo.observe(capTrust, { childList: true, characterData: true, subtree: true });
    mirrorCannot();
  }

  /* ---- the sidebar's Port an app ---------------------------------------- */
  const sidePort = $("sidePort");
  if (sidePort) sidePort.addEventListener("click", () => {
    // From any page: back to Home with Port chosen, then the folder picker.
    const homeRow = document.querySelector('#side .side-row[data-side="home"]');
    if (homeRow && $("viewHome") && $("viewHome").classList.contains("hidden")) homeRow.click();
    setMode("port");
    startPort();
  });
})();
