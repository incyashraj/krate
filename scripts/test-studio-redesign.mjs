#!/usr/bin/env node
// Studio's design layer (studio/ui/redesign.css and redesign.js) sits on top
// of style.css, session.css and app.js. Three ways it can break the page
// without any test noticing, each one seen while it was being built:
//
//  1. Two @keyframes with one name: the later wins everywhere, so a spinner's
//     rotate turned the build card's outline into a spinning slab.
//  2. An icon used as #i-name that the sprite in index.html does not define:
//     it renders as nothing.
//  3. A top-level name in redesign.js: app.js and bridge.js share one global
//     scope in a browser tab, and a clash blanks the page.
//
//   node scripts/test-studio-redesign.mjs [--self-test]
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

function keyframeNames(css) { return [...css.matchAll(/@keyframes\s+([\w-]+)/g)].map((m) => m[1]); }

export function check({ redesignCss, oldCss, redesignJs, indexHtml }) {
  const bad = [];
  const mine = keyframeNames(redesignCss);
  const seen = new Set();
  for (const n of mine) { if (seen.has(n)) bad.push(`@keyframes ${n} is defined twice in redesign.css`); seen.add(n); }
  const old = new Set(keyframeNames(oldCss));
  for (const n of seen) if (old.has(n)) bad.push(`@keyframes ${n} in redesign.css also exists in style.css/session.css`);
  const sprite = new Set([...indexHtml.matchAll(/<symbol id="(i-[\w-]+)"/g)].map((m) => m[1]));
  const used = new Set([...redesignJs.matchAll(/ico\("([\w-]+)"/g)].map((m) => 'i-' + m[1]));
  for (const id of used) if (!sprite.has(id)) bad.push(`redesign.js uses #${id}, which the sprite in index.html does not define`);
  try { new vm.Script(redesignJs); } catch (e) { bad.push(`redesign.js does not parse: ${e.message}`); }
  const ctx = vm.createContext({ window: {}, document: { readyState: 'loading', addEventListener() {}, getElementById() { return null; }, querySelector() { return null; }, querySelectorAll() { return []; }, createElement() { return { style: {}, classList: { add() {} }, setAttribute() {}, appendChild() {} }; }, body: { appendChild() {}, classList: { contains() { return false; }, toggle() {} } } } });
  const before = new Set(Object.getOwnPropertyNames(ctx));
  try { vm.runInContext(redesignJs.replace(/^\(function \(\) \{/m, '(function () { return;'), ctx); } catch (e) { /* the body is skipped; only the shape matters */ }
  const after = Object.getOwnPropertyNames(ctx).filter((n) => !before.has(n));
  if (after.length) bad.push(`redesign.js declares top-level names: ${after.join(', ')}`);
  if (!/^\(function \(\) \{/m.test(redesignJs)) bad.push('redesign.js is not wrapped in one function');
  return bad;
}

if (process.argv.includes('--self-test')) {
  const base = { redesignCss: '@keyframes a{}', oldCss: '@keyframes b{}', redesignJs: '(function () {\n ico("x");\n})();', indexHtml: '<symbol id="i-x">' };
  const cases = [
    [{}, 0],
    [{ redesignCss: '@keyframes a{} @keyframes a{}' }, 1],
    [{ oldCss: '@keyframes a{}' }, 1],
    [{ indexHtml: '<symbol id="i-y">' }, 1],
    [{ redesignJs: 'var leak = 1;\n(function () {\n})();' }, 1],
  ];
  let fails = 0;
  for (const [over, want] of cases) {
    const got = check({ ...base, ...over }).length;
    if ((got > 0) !== (want > 0)) { fails++; console.log('self-test FAIL', JSON.stringify(over), got); }
  }
  console.log(fails ? `${fails} self-test case(s) failed` : 'ok  self-test: every hazard is caught');
  process.exit(fails ? 1 : 0);
}

const read = (p) => readFileSync(p, 'utf8');
const bad = check({
  redesignCss: read('studio/ui/redesign.css'),
  oldCss: read('studio/ui/style.css') + read('studio/ui/session.css'),
  redesignJs: read('studio/ui/redesign.js'),
  indexHtml: read('studio/ui/index.html'),
});
for (const b of bad) console.log('FAIL', b);
console.log(bad.length ? `${bad.length} problem(s) in Studio's design layer` : 'ok  Studio design layer: keyframes unique, icons defined, no global names');
process.exit(bad.length ? 1 : 0);
