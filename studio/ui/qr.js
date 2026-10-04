/* A QR code for a share link, drawn as SVG.
 *
 * Byte mode, error correction level M, versions 1 to 6: up to 106 bytes,
 * which every hub link fits in many times over. Longer text gets no code
 * (null) rather than a wrong one. Follows ISO/IEC 18004 the way Project
 * Nayuki's reference encoder lays it out.
 *
 * One name on window, krQrSvg, so it never clashes with app.js or the
 * web bridge (they share one global scope in a tab).
 */
(function () {
  "use strict";
  // Version: [error-correction codewords per block, blocks, data codewords per block]
  const M = { 1: [10, 1, 16], 2: [16, 1, 28], 3: [26, 1, 44], 4: [18, 2, 32], 5: [24, 2, 43], 6: [16, 4, 27] };
  const ALIGN = { 1: [], 2: [6, 18], 3: [6, 22], 4: [6, 26], 5: [6, 30], 6: [6, 34] };

  function mul(x, y) {
    let z = 0;
    for (let i = 7; i >= 0; i--) { z = (z << 1) ^ ((z >>> 7) * 0x11d); z ^= ((y >>> i) & 1) * x; }
    return z & 0xff;
  }
  function divisor(degree) {
    const r = new Array(degree).fill(0); r[degree - 1] = 1;
    let root = 1;
    for (let i = 0; i < degree; i++) {
      for (let j = 0; j < r.length; j++) { r[j] = mul(r[j], root); if (j + 1 < r.length) r[j] ^= r[j + 1]; }
      root = mul(root, 0x02);
    }
    return r;
  }
  function remainder(data, div) {
    const r = new Array(div.length).fill(0);
    for (const b of data) {
      const f = b ^ r.shift(); r.push(0);
      div.forEach((c, i) => { r[i] ^= mul(c, f); });
    }
    return r;
  }

  function encode(text) {
    const bytes = Array.from(new TextEncoder().encode(String(text)));
    let ver = 0;
    for (let v = 1; v <= 6; v++) { const [, n, d] = M[v]; if (4 + 8 + bytes.length * 8 <= n * d * 8) { ver = v; break; } }
    if (!ver) return null;
    const [ecn, nb, dn] = M[ver];
    const cap = nb * dn * 8;
    const bits = [];
    const put = (val, len) => { for (let i = len - 1; i >= 0; i--) bits.push((val >>> i) & 1); };
    put(4, 4); put(bytes.length, 8); bytes.forEach((b) => put(b, 8));
    put(0, Math.min(4, cap - bits.length));
    put(0, (8 - bits.length % 8) % 8);
    for (let pad = 0xec; bits.length < cap; pad ^= 0xec ^ 0x11) put(pad, 8);
    const data = [];
    for (let i = 0; i < bits.length; i += 8) data.push(bits.slice(i, i + 8).reduce((a, b) => (a << 1) | b, 0));
    // Blocks, each with its error correction, then interleaved column-wise.
    const div = divisor(ecn), blocks = [], ecs = [];
    for (let b = 0; b < nb; b++) { const d = data.slice(b * dn, (b + 1) * dn); blocks.push(d); ecs.push(remainder(d, div)); }
    const all = [];
    for (let i = 0; i < dn; i++) blocks.forEach((d) => all.push(d[i]));
    for (let i = 0; i < ecn; i++) ecs.forEach((e) => all.push(e[i]));

    const size = ver * 4 + 17;
    const mod = Array.from({ length: size }, () => new Array(size).fill(false));
    const fn = Array.from({ length: size }, () => new Array(size).fill(false));
    const set = (x, y, dark) => { mod[y][x] = dark; fn[y][x] = true; };
    for (let i = 0; i < size; i++) { set(6, i, i % 2 === 0); set(i, 6, i % 2 === 0); }
    const finder = (cx, cy) => {
      for (let dy = -4; dy <= 4; dy++) for (let dx = -4; dx <= 4; dx++) {
        const x = cx + dx, y = cy + dy, d = Math.max(Math.abs(dx), Math.abs(dy));
        if (x >= 0 && x < size && y >= 0 && y < size) set(x, y, d !== 2 && d !== 4);
      }
    };
    finder(3, 3); finder(size - 4, 3); finder(3, size - 4);
    const al = ALIGN[ver], last = al.length - 1;
    al.forEach((ay, i) => al.forEach((ax, j) => {
      if ((i === 0 && j === 0) || (i === 0 && j === last) || (i === last && j === 0)) return;
      for (let dy = -2; dy <= 2; dy++) for (let dx = -2; dx <= 2; dx++) set(ax + dx, ay + dy, Math.max(Math.abs(dx), Math.abs(dy)) !== 1);
    }));
    const format = (mask) => {
      const d = (0 << 3) | mask; // level M is 00
      let rem = d;
      for (let i = 0; i < 10; i++) rem = (rem << 1) ^ ((rem >>> 9) * 0x537);
      const f = ((d << 10) | rem) ^ 0x5412;
      const bit = (i) => ((f >>> i) & 1) !== 0;
      for (let i = 0; i <= 5; i++) set(8, i, bit(i));
      set(8, 7, bit(6)); set(8, 8, bit(7)); set(7, 8, bit(8));
      for (let i = 9; i < 15; i++) set(14 - i, 8, bit(i));
      for (let i = 0; i < 8; i++) set(size - 1 - i, 8, bit(i));
      for (let i = 8; i < 15; i++) set(8, size - 15 + i, bit(i));
      set(8, size - 8, true);
    };
    format(0);
    // The data, two columns at a time, snaking up and down from the right.
    let k = 0;
    for (let right = size - 1; right >= 1; right -= 2) {
      if (right === 6) right = 5;
      for (let v = 0; v < size; v++) for (let j = 0; j < 2; j++) {
        const x = right - j, up = ((right + 1) & 2) === 0, y = up ? size - 1 - v : v;
        if (!fn[y][x] && k < all.length * 8) { mod[y][x] = ((all[k >>> 3] >>> (7 - (k & 7))) & 1) !== 0; k++; }
      }
    }
    const MASKS = [
      (x, y) => (x + y) % 2 === 0, (x, y) => y % 2 === 0, (x) => x % 3 === 0, (x, y) => (x + y) % 3 === 0,
      (x, y) => (Math.floor(x / 3) + Math.floor(y / 2)) % 2 === 0, (x, y) => (x * y) % 2 + (x * y) % 3 === 0,
      (x, y) => ((x * y) % 2 + (x * y) % 3) % 2 === 0, (x, y) => ((x + y) % 2 + (x * y) % 3) % 2 === 0,
    ];
    const flip = (m) => { for (let y = 0; y < size; y++) for (let x = 0; x < size; x++) if (!fn[y][x] && MASKS[m](x, y)) mod[y][x] = !mod[y][x]; };
    // Runs of one colour, 2x2 blocks and the dark share: enough of the
    // standard's penalty to pick a mask that scans well.
    const penalty = () => {
      let p = 0, dark = 0;
      for (let y = 0; y < size; y++) {
        for (const line of [(i) => mod[y][i], (i) => mod[i][y]]) {
          let run = 1;
          for (let i = 1; i < size; i++) { if (line(i) === line(i - 1)) { run++; if (run === 5) p += 3; else if (run > 5) p++; } else run = 1; }
        }
        for (let x = 0; x < size; x++) {
          if (mod[y][x]) dark++;
          if (x < size - 1 && y < size - 1 && mod[y][x] === mod[y][x + 1] && mod[y][x] === mod[y + 1][x] && mod[y][x] === mod[y + 1][x + 1]) p += 3;
        }
      }
      return p + Math.floor(Math.abs(dark * 20 - size * size * 10) / (size * size)) * 10;
    };
    let best = 0, low = Infinity;
    for (let m = 0; m < 8; m++) { flip(m); format(m); const s = penalty(); if (s < low) { low = s; best = m; } flip(m); }
    flip(best); format(best);
    return { size, mod };
  }

  window.krQrSvg = function (text, cls) {
    const q = encode(text);
    if (!q) return "";
    const n = q.size + 8;
    let d = "";
    for (let y = 0; y < q.size; y++) for (let x = 0; x < q.size; x++) if (q.mod[y][x]) d += `M${x + 4} ${y + 4}h1v1h-1z`;
    return `<svg class="${cls || ""}" viewBox="0 0 ${n} ${n}" shape-rendering="crispEdges" role="img" aria-label="QR code for the link"><rect width="${n}" height="${n}" fill="#fff"/><path d="${d}" fill="#111"/></svg>`;
  };
})();
