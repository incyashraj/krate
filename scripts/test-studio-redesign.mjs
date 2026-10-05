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
//  4. A bare class rule in redesign.css (".live {", ".kr-in {") whose class
//     the scripts also add as a STATE: the rule then lands on every element
//     in that state. The IDE's ".live" badge painted a green pill behind
//     app.js's live thinking line, and a share-sheet field's ".kr-in" put a
//     grey 40px box round every new line of the conversation.
//
//   node scripts/test-studio-redesign.mjs [--self-test]
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

function keyframeNames(css) { return [...css.matchAll(/@keyframes\s+([\w-]+)/g)].map((m) => m[1]); }

// Classes a script adds or toggles at runtime: classList.add/toggle/remove("x").
function stateClasses(js) {
  const out = new Set();
  for (const m of js.matchAll(/classList\.(?:add|toggle|remove)\(([^)]*)\)/g)) for (const q of m[1].matchAll(/["']([\w-]+)["']/g)) out.add(q[1]);
  return out;
}
// Top-level rules that start with one bare class: ".x {", ".x.y", ".x i" ...
function bareClassRules(css) {
  const out = new Set();
  const flat = css.replace(/\/\*[\s\S]*?\*\//g, '');
  for (const m of flat.matchAll(/(^|[}\n])\s*((?:[^{}@]+))\{/g)) for (const sel of m[2].split(',')) { const t = sel.trim(); const c = t.match(/^\.([\w-]+)(?=[\s.:\[>{]|$)/); if (c) out.add(c[1]); }
  return out;
}
export function check({ redesignCss, oldCss, redesignJs, indexHtml, scripts = '' }) {
  const bad = [];
  const states = stateClasses(redesignJs + '\n' + scripts);
  // A class the CSS styles ON PURPOSE as a state says so: /* kr-state: a b */
  for (const m of redesignCss.matchAll(/kr-state:([^*]*)\*\//g)) for (const n of m[1].trim().split(/\s+/)) states.delete(n.replace(/--.*$/, ''));
  for (const c of bareClassRules(redesignCss)) if (states.has(c)) bad.push(`redesign.css styles .${c} at the top level, and a script adds "${c}" as a state: scope the rule (an id or a parent) or rename it`);
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
    [{ redesignCss: '@keyframes a{} .live { background: green; }', scripts: 'el.classList.add("live");' }, 1],
    [{ redesignCss: '@keyframes a{} #viewIde .live { background: green; }', scripts: 'el.classList.add("live");' }, 0],
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
  scripts: read('studio/ui/app.js') + read('studio/ui/ide.js'),
});
for (const b of bad) console.log('FAIL', b);
console.log(bad.length ? `${bad.length} problem(s) in Studio's design layer` : 'ok  Studio design layer: keyframes unique, icons defined, no global names');
process.exit(bad.length ? 1 : 0);
