/* What a label printer would print, drawn: ZPL (Zebra) and ESC/POS (receipt)
 * turned into a picture, for the workbench's simulated printer (IC-995).
 *
 * Nothing here talks to a printer or the network: it reads the bytes an app
 * sent to Krate's pretend printer and paints them on a canvas at the
 * printer's own 203 dots per inch, real Code 128 barcodes and QR codes
 * included, so a label for a Zebra is checked with no Zebra.
 *
 * One function in one global, window.krLabels, because the workbench reads
 * it and app.js and bridge.js share one scope in a tab (scope-test.mjs).
 * The Code 128 table is checked by labels-test.mjs.
 */
(function (root) {
  "use strict";

  /* ---- Code 128 -------------------------------------------------------- */
  // Each symbol as 11 modules, 1 = bar; the stop symbol has 13.
  const C128 = [
    "11011001100", "11001101100", "11001100110", "10010011000", "10010001100", "10001001100", "10011001000", "10011000100",
    "10001100100", "11001001000", "11001000100", "11000100100", "10110011100", "10011011100", "10011001110", "10111001100",
    "10011101100", "10011100110", "11001110010", "11001011100", "11001001110", "11011100100", "11001110100", "11101101110",
    "11101001100", "11100101100", "11100100110", "11101100100", "11100110100", "11100110010", "11011011000", "11011000110",
    "11000110110", "10100011000", "10001011000", "10001000110", "10110001000", "10001101000", "10001100010", "11010001000",
    "11000101000", "11000100010", "10110111000", "10110001110", "10001101110", "10111011000", "10111000110", "10001110110",
    "11101110110", "11010001110", "11000101110", "11011101000", "11011100010", "11011101110", "11101011000", "11101000110",
    "11100010110", "11101101000", "11101100010", "11100011010", "11101111010", "11001000010", "11110001010", "10100110000",
    "10100001100", "10010110000", "10010000110", "10000101100", "10000100110", "10110010000", "10110000100", "10011010000",
    "10011000010", "10000110100", "10000110010", "11000010010", "11001010000", "11110111010", "11000010100", "10001111010",
    "10100111100", "10010111100", "10010011110", "10111100100", "10011110100", "10011110010", "11110100100", "11110010100",
    "11110010010", "11011011110", "11011110110", "11110110110", "10101111000", "10100011110", "10001011110", "10111101000",
    "10111100010", "11110101000", "11110100010", "10111011110", "10111101110", "11101011110", "11110101110", "11010000100",
    "11010010000", "11010011100",
  ];
  const C128_STOP = "1100011101011";
  const START_B = 104, START_C = 105, CODE_B = 100, CODE_C = 99;

  // Code set B for text, C for runs of digits (as a printer's auto mode does).
  function code128(text) {
    const s = String(text).replace(/[^\x20-\x7e]/g, "");
    const values = [];
    let i = 0, set = null;
    const digitsAt = (k) => { let n = 0; while (k + n < s.length && /\d/.test(s[k + n])) n++; return n; };
    while (i < s.length) {
      const run = digitsAt(i);
      if (run >= 4 || (run >= 2 && run === s.length - i && set === "C")) {
        const take = run - (run % 2);
        if (set !== "C") { values.push(set === null ? START_C : CODE_C); set = "C"; }
        for (let k = 0; k < take; k += 2) values.push(Number(s.slice(i + k, i + k + 2)));
        i += take;
        continue;
      }
      if (set !== "B") { values.push(set === null ? START_B : CODE_B); set = "B"; }
      values.push(s.charCodeAt(i) - 32);
      i++;
    }
    if (!values.length) return "";
    let sum = values[0];
    for (let k = 1; k < values.length; k++) sum += values[k] * k;
    values.push(sum % 103);
    return values.map((v) => C128[v]).join("") + C128_STOP;
  }

  /* ---- ZPL ------------------------------------------------------------- */
  // The commands a label app sends, enough for text, boxes, lines, Code 128
  // and QR: ^XA ^XZ ^PW ^LL ^LH ^FO ^FT ^A ^CF ^FB ^FD ^FS ^FR ^GB ^BY ^BC
  // ^BQ. Anything else is skipped and named, never guessed at.
  function zplLabels(source) {
    const text = String(source);
    const labels = [];
    const re = /\^XA([\s\S]*?)(?:\^XZ|$)/g;
    let m;
    while ((m = re.exec(text))) if (m[1].trim()) labels.push(m[1]);
    return labels;
  }

  function parseZpl(body) {
    const cmds = [];
    const re = /([\^~])([A-Z0-9@]{1,2})([^\^~]*)/g;
    let m;
    while ((m = re.exec(body))) {
      let code = m[2], rest = m[3];
      // ^A takes a font letter right after it: ^A0N,30,30 or ^ADN,18,10.
      if (code[0] === "A" && code.length === 2) { rest = code[1] + rest; code = "A"; }
      cmds.push({ code, args: rest });
    }
    return cmds;
  }

  function drawZpl(body, opts) {
    const dpmm = 8; // 203 dpi
    const cmds = parseZpl(body);
    const st = { w: 812, h: 0, lh: [0, 0] };
    for (const c of cmds) {
      if (c.code === "PW") st.w = Number(c.args) || st.w;
      if (c.code === "LL") st.h = Number(c.args) || 0;
    }
    // Lay it out once to find how tall the label is when ^LL does not say.
    const ops = [];
    let at = [0, 0], font = { h: 30, w: 0 }, block = null, by = { w: 2, ratio: 3, h: 10 * dpmm }, field = null, reverse = false, hex = null, unknown = new Set();
    // ^FH: the field's data carries escapes like _41 (hex bytes), the
    // indicator character given (default underscore).
    const unhex = (data, mark) => {
      const esc = mark.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
      const bytes = [];
      data.replace(new RegExp(`${esc}([0-9A-Fa-f]{2})|[\\s\\S]`, "g"), (m, h) => { if (h) bytes.push(parseInt(h, 16)); else for (const b of new TextEncoder().encode(m)) bytes.push(b); return ""; });
      return new TextDecoder().decode(new Uint8Array(bytes));
    };
    for (const c of cmds) {
      const a = c.args.split(",");
      switch (c.code) {
        case "LH": st.lh = [Number(a[0]) || 0, Number(a[1]) || 0]; break;
        case "FO": case "FT": at = [(Number(a[0]) || 0) + st.lh[0], (Number(a[1]) || 0) + st.lh[1]]; field = { kind: "text", baseline: c.code === "FT" }; break;
        case "A": font = { h: Number(a[1]) || font.h, w: Number(a[2]) || 0 }; break;
        case "CF": font = { h: Number(a[1]) || font.h, w: Number(a[2]) || 0 }; break;
        case "FB": block = { w: Number(a[0]) || 0, lines: Number(a[1]) || 1, gap: Number(a[2]) || 0, just: a[3] || "L" }; break;
        case "FR": reverse = true; break;
        case "BY": by = { w: Number(a[0]) || by.w, ratio: Number(a[1]) || by.ratio, h: Number(a[2]) || by.h }; break;
        case "BC": field = { kind: "c128", h: Number(a[1]) || by.h, line: (a[2] || "Y").trim() !== "N", above: (a[3] || "N").trim() === "Y" }; break;
        case "BQ": field = { kind: "qr", mag: Number(a[2]) || 3 }; break;
        case "GB": {
          const w = Number(a[0]) || 1, h = Number(a[1]) || 1, t = Math.min(Number(a[2]) || 1, w, h);
          ops.push({ op: "box", x: at[0], y: at[1], w, h, t, white: (a[3] || "B").trim() === "W" });
          break;
        }
        case "FH": hex = (c.args || "_").charAt(0) || "_"; break;
        case "FD": case "FV": {
          const data = hex ? unhex(c.args, hex) : c.args;
          if (field) ops.push({ op: field.kind, x: at[0], y: at[1], data, font: { ...font }, block, by: { ...by }, f: field, reverse });
          break;
        }
        case "FS": field = null; block = null; reverse = false; hex = null; break;
        case "XA": case "XZ": case "PW": case "LL": case "CI": case "PQ": case "MM": case "MN": case "MD": case "PR": case "LR": case "PO": case "LS": case "JM": case "FX": break;
        default: unknown.add("^" + c.code);
      }
    }
    let bottom = 0;
    for (const o of ops) bottom = Math.max(bottom, o.y + (o.op === "box" ? o.h : o.op === "c128" ? o.f.h + 30 : o.op === "qr" ? 200 : o.font.h * (o.block ? o.block.lines : 1)));
    const H = st.h || Math.max(200, bottom + 24);
    const scale = Math.min(1, (opts && opts.maxWidth ? opts.maxWidth : st.w) / st.w);
    const cv = document.createElement("canvas");
    const dpr = (opts && opts.dpr) || 2;
    cv.width = Math.round(st.w * scale * dpr); cv.height = Math.round(H * scale * dpr);
    cv.style.width = `${Math.round(st.w * scale)}px`; cv.style.height = `${Math.round(H * scale)}px`;
    const g = cv.getContext("2d");
    g.scale(scale * dpr, scale * dpr);
    g.fillStyle = "#fff"; g.fillRect(0, 0, st.w, H);
    g.fillStyle = "#000"; g.strokeStyle = "#000"; g.textBaseline = "top";
    for (const o of ops) {
      if (o.op === "box") {
        g.fillStyle = o.white ? "#fff" : "#000";
        if (o.t >= Math.min(o.w, o.h)) g.fillRect(o.x, o.y, o.w, o.h);
        else { g.fillRect(o.x, o.y, o.w, o.t); g.fillRect(o.x, o.y + o.h - o.t, o.w, o.t); g.fillRect(o.x, o.y, o.t, o.h); g.fillRect(o.x + o.w - o.t, o.y, o.t, o.h); }
        g.fillStyle = "#000";
      } else if (o.op === "text") {
        const h = o.font.h;
        g.font = `600 ${Math.round(h * 0.86)}px "Helvetica Neue", Arial, sans-serif`;
        const y = o.f.baseline ? o.y - h : o.y;
        const lines = o.block ? wrap(g, o.data.replace(/\\&/g, "\n"), o.block.w || st.w - o.x).slice(0, o.block.lines) : [o.data];
        lines.forEach((line, k) => {
          let x = o.x;
          if (o.block && o.block.w) {
            const tw = g.measureText(line).width;
            if (/C/i.test(o.block.just)) x = o.x + (o.block.w - tw) / 2;
            else if (/R/i.test(o.block.just)) x = o.x + o.block.w - tw;
          }
          const ty = y + k * (h + (o.block ? o.block.gap : 0));
          if (o.reverse) { const tw = g.measureText(line).width; g.fillRect(x - 4, ty - 2, tw + 8, h + 4); g.fillStyle = "#fff"; g.fillText(line, x, ty); g.fillStyle = "#000"; }
          else g.fillText(line, x, ty);
        });
      } else if (o.op === "c128") {
        const bits = code128(o.data);
        let x = o.x;
        const y = o.f.baseline ? o.y - o.f.h : o.y;
        for (const b of bits) { if (b === "1") g.fillRect(x, y, o.by.w, o.f.h); x += o.by.w; }
        if (o.f.line) {
          g.font = `500 ${Math.round(Math.max(18, o.by.w * 9))}px "Menlo", monospace`;
          const tw = g.measureText(o.data).width, cx = o.x + (bits.length * o.by.w - tw) / 2;
          g.fillText(o.data, cx, o.f.above ? y - Math.max(18, o.by.w * 9) - 4 : y + o.f.h + 6);
        }
      } else if (o.op === "qr") {
        // ^FDQA,data: the error level and mode come first.
        const data = o.data.replace(/^[HQML]A,/, "").replace(/^[HQML]M,[NAB]?/, "");
        const qr = root.krQr && root.krQr(data);
        if (qr) {
          const n = qr.length, s = o.f.mag;
          const y = o.y + 10;
          for (let r = 0; r < n; r++) for (let c = 0; c < n; c++) if (qr[r][c]) g.fillRect(o.x + 10 + c * s, y + r * s, s, s);
        }
      }
    }
    return { canvas: cv, width: st.w, height: H, unknown: [...unknown] };
  }

  function wrap(g, text, width) {
    const out = [];
    for (const para of String(text).split("\n")) {
      let line = "";
      for (const word of para.split(/\s+/)) {
        const next = line ? line + " " + word : word;
        if (line && g.measureText(next).width > width) { out.push(line); line = word; } else line = next;
      }
      out.push(line);
    }
    return out;
  }

  /* ---- ESC/POS --------------------------------------------------------- */
  // A receipt is mostly text with control codes around it: keep the text and
  // the line breaks, drop the codes, mark a cut.
  function escposText(bytes) {
    const b = bytes instanceof Uint8Array ? bytes : new TextEncoder().encode(String(bytes));
    let out = "", i = 0;
    while (i < b.length) {
      const c = b[i];
      if (c === 0x1b || c === 0x1d) {
        const cmd = b[i + 1];
        if (c === 0x1d && cmd === 0x56) { out += "\n✂ - - - - - - - - - - - - - -\n"; i += 3; continue; }
        if (c === 0x1b && (cmd === 0x40)) { i += 2; continue; }
        if (c === 0x1d && cmd === 0x6b) { // barcode: GS k m n d1..dn
          const m = b[i + 2], n = b[i + 3]; i += 4 + (m > 6 ? n : 0); continue;
        }
        i += 3; continue;
      }
      if (c === 0x0a) { out += "\n"; i++; continue; }
      if (c >= 0x20 && c < 0x7f) out += String.fromCharCode(c);
      else if (c >= 0x80) out += String.fromCharCode(c);
      i++;
    }
    return out.replace(/\n{3,}/g, "\n\n").trim();
  }

  function looksLikeZpl(text) { return /\^XA/.test(text); }

  root.krLabels = { code128, zplLabels, drawZpl, escposText, looksLikeZpl, C128, C128_STOP };
})(typeof window !== "undefined" ? window : globalThis);
