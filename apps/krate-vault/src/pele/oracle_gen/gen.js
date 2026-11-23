// Renders each source with Pele's build and writes [{name, src, svg|error}].
global.window = global; global.self = global;
const fs = require('fs');
const src = fs.readFileSync(process.env.HOME + '/krate-bench/vault-render/pele/pele.min.js', 'utf8');
eval(src + ';global.pele=pele;');
const cases = JSON.parse(fs.readFileSync(process.argv[2], 'utf8'));
const out = [];
for (const c of cases) {
  try {
    const o = Object.assign({ idPrefix: 'x' }, c.opts || {});
    if (o.iconsMap) { const m = o.iconsMap; o.icons = (n) => m[n]; delete o.iconsMap; }
    const r = pele.render(c.src, o);
    out.push(Object.assign({ name: c.name, src: c.src, svg: r.svg }, c.opts ? { opts: c.opts } : {}));
  } catch (e) {
    out.push(Object.assign({ name: c.name, src: c.src, error: String(e.message) }, c.opts ? { opts: c.opts } : {}));
  }
}
fs.writeFileSync(process.argv[3], JSON.stringify(out, null, 0));
console.log(out.length, 'cases,', out.filter((c) => c.error).length, 'errors');
