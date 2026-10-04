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
