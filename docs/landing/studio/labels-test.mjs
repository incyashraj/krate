// The label preview's Code 128 table follows the symbology's own rules, so
// a barcode the workbench draws is one a scanner reads (IC-995).
//
//   node docs/landing/studio/labels-test.mjs
import { readFileSync } from "node:fs";
const src = readFileSync(new URL("../../../studio/ui/labels.js", import.meta.url), "utf8");
const scope = {};
new Function("globalThis", src.replace("typeof window !== \"undefined\" ? window : globalThis", "globalThis"))(scope);
const { C128, C128_STOP, code128, zplLabels, escposText } = scope.krLabels;
let bad = 0;
const fail = (m) => { bad++; console.log("FAIL", m); };
if (C128.length !== 106) fail(`106 symbols, not ${C128.length}`);
if (new Set(C128).size !== C128.length) fail("every symbol is different");
C128.forEach((p, v) => {
  if (!/^[01]{11}$/.test(p)) return fail(`symbol ${v} is 11 modules`);
  const runs = p.match(/1+|0+/g);
  if (runs.length !== 6 || p[0] !== "1") fail(`symbol ${v}: three bars and three spaces, bar first`);
  if (runs.some((r) => r.length > 4)) fail(`symbol ${v}: no element wider than 4`);
  const bars = runs.filter((_, i) => i % 2 === 0).reduce((n, r) => n + r.length, 0);
  if (bars % 2) fail(`symbol ${v}: bar modules even (${bars})`);
});
if (C128_STOP !== "1100011101011") fail("the stop symbol");
// Start B, the data, the check symbol, stop: 11 modules each plus 13.
const wiki = code128("Wikipedia");
if (wiki.length !== 11 * (1 + 9 + 1) + 13) fail(`"Wikipedia" length ${wiki.length}`);
// Its check symbol (Wikipedia's own worked example): value 88.
if (wiki.slice(11 * 10, 11 * 11) !== C128[88]) fail(`"Wikipedia" check symbol`);
// Digits pack in pairs (set C): 12 digits are 6 symbols, not 12.
if (code128("123456789012").length !== 11 * (1 + 6 + 1) + 13) fail("digits use set C");
if (zplLabels("^XA^FO10,10^FDone^FS^XZ^XA^FDtwo^FS^XZ").length !== 2) fail("two labels in one job");
if (!/Total 12\.50/.test(escposText(new Uint8Array([0x1b, 0x40, ...new TextEncoder().encode("Total 12.50\n"), 0x1d, 0x56, 0x00])))) fail("receipt text kept");
if (bad) process.exit(1);
console.log("ok  Code 128 table follows the symbology; ZPL and receipt text parse");
