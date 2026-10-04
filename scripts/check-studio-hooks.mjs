// Run from the repo root after any change to studio/ui:
//   node scripts/check-studio-hooks.mjs            (compares against the baseline in scripts/studio-ui-hooks.json)
//   node scripts/check-studio-hooks.mjs --baseline (records the current hooks as the baseline; only when a hook is deliberately added or retired)
// It fails if an element id, backend command, event, state class or data-* attribute that
// studio/ui/app.js or the web bridge relies on has disappeared from the UI.
import fs from 'node:fs';
const read = p => fs.readFileSync(p, 'utf8');
const app = read('studio/ui/app.js'), html = read('studio/ui/index.html'), bridge = read('docs/landing/studio/bridge.js');
const all = (re, s) => new Set([...s.matchAll(re)].map(m => m[1]));
const ids = s => new Set([...all(/getElementById\(\s*['"]([\w-]+)['"]/g, s), ...all(/\$\(\s*['"]([\w-]+)['"]\s*\)/g, s), ...all(/querySelector(?:All)?\(\s*['"`]#([\w-]+)/g, s)]);
const now = {
  commands: [...all(/invoke\(\s*['"](\w+)/g, app)].sort(),
  events: [...all(/listen\(\s*['"]([\w:-]+)/g, app)].sort(),
  appIds: [...ids(app)].sort(),
  bridgeIds: [...ids(bridge)].sort(),
  htmlIds: [...all(/id="([\w-]+)"/g, html)].sort(),
  classes: [...all(/classList\.(?:add|remove|toggle|contains)\(\s*['"]([\w-]+)/g, app)].sort(),
  data: [...new Set([...all(/dataset\.(\w+)/g, app), ...all(/data-([\w-]+)/g, app)])].sort(),
};
const BASE = 'scripts/studio-ui-hooks.json';
if (process.argv.includes('--baseline')) { fs.writeFileSync(BASE, JSON.stringify(now, null, 1)); console.log('baseline written:', Object.entries(now).map(([k, v]) => `${k} ${v.length}`).join(', ')); process.exit(0); }
const base = JSON.parse(read(BASE)); let bad = 0;
for (const k of Object.keys(base)) { const gone = base[k].filter(x => !now[k].includes(x)); if (gone.length) { bad += gone.length; console.log(`MISSING ${k}: ${gone.join(' ')}`); } }
// every id the scripts look up must exist in the page, unless the page never had it (made at runtime)
const runtimeMade = base.appIds.filter(i => !base.htmlIds.includes(i));
const lost = [...new Set([...now.appIds, ...now.bridgeIds])].filter(i => !now.htmlIds.includes(i) && !runtimeMade.includes(i) && !base.bridgeIds.filter(b => !base.htmlIds.includes(b)).includes(i));
if (lost.length) { bad += lost.length; console.log('LOOKED UP BUT NOT IN index.html:', lost.join(' ')); }
console.log(bad ? `FAIL: ${bad} hook(s) lost` : 'OK: every hook is still there'); process.exit(bad ? 1 : 0);
