/* Krate's own motion for anything that loads (the 2026-10 design).
 *
 * No spinners. The mark is three layers in a box, so when something loads
 * the layers move: they part and close (working), drop in and stack
 * (opening), lift like a lid (an app opening), fill from the bottom (a file
 * coming in), slip out of line (offline). Under 0.4 s nothing shows; past
 * 3 s a plain line says what is happening; a loader ends on the thing
 * itself, never on a blank.
 *
 * Two names on window, both prefixed, so nothing clashes with app.js or the
 * web bridge (they share one global scope in a tab):
 *   krIso(size, cls, mono)  the mark as SVG; cls: breathe | stack | gfill
 *   krSwap(box, html)       a line of text that changes: old rises out,
 *                           new rises in
 */
(function () {
  "use strict";
  window.krIso = function (size, cls, mono) {
    const layers = [2, 1, 0].map((k) => {
      const y = 6 + k * 15;
      return `<g class="ly" style="--k:${k}"><polygon class="tp" points="50,${y} 92,${y + 21} 50,${y + 42} 8,${y + 21}"/>` +
        `<polygon class="lf" points="8,${y + 21} 50,${y + 42} 50,${y + 54} 8,${y + 33}"/>` +
        `<polygon class="rt" points="50,${y + 42} 92,${y + 21} 92,${y + 33} 50,${y + 54}"/></g>`;
    }).join("");
    return `<svg class="iso ${cls || ""}${mono ? " mono" : ""}" width="${size}" height="${size}" viewBox="0 0 100 100" aria-hidden="true" focusable="false">${layers}</svg>`;
  };
  window.krSwap = function (box, html) {
    if (!box) return;
    const cur = box.lastElementChild;
    if (cur && cur.innerHTML === html) return;
    if (cur) { cur.classList.remove("sw-in"); cur.classList.add("sw-out"); setTimeout(() => cur.remove(), 500); }
    const n = document.createElement("span");
    n.className = "sw-in";
    n.innerHTML = html;
    box.appendChild(n);
  };
  // Stack the layers in, bottom first: the mark arriving.
  window.krStack = function (root, delay) {
    if (!root) return;
    [...root.querySelectorAll(".iso.stack .ly")].forEach((l, i) => setTimeout(() => l.classList.add("on"), (delay || 120) + i * 190));
  };
})();
