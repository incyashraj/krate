// Run Pele on a .mmd file and record every call of its layout `On`:
// the nodes, edges, direction and options it was given, and what it gave
// back (node x/y/w/h, edge route and label point, the size).
global.window = global; global.self = global;
const fs = require('fs');
let src = fs.readFileSync(__dirname + '/pele.min.js', 'utf8');
src = src.replace('function On(e,t,n,r){', 'function On(e,t,n,r){return globalThis.__wrapOn(On_orig,e,t,n,r)}function On_orig(e,t,n,r){');
const calls = [];
globalThis.__wrapOn = (f, e, t, n, r) => {
  const clone = (o) => JSON.parse(JSON.stringify(o, (k, v) => (typeof v === 'number' && !isFinite(v)) ? String(v) : v));
  const input = { nodes: clone(e), edges: clone(t), dir: n, opts: clone(r) };
  const size = f(e, t, n, r);
  calls.push({ input, output: { size: clone(size), nodes: e.map(x => ({ x: x.x, y: x.y, w: x.w, h: x.h })), edges: t.map(x => ({ route: x.route.slice(), labelX: x.labelX, labelY: x.labelY, src: x.src, dst: x.dst })) } });
  return size;
};
eval(src + ';globalThis.pele=pele;');
const text = fs.readFileSync(process.argv[2], 'utf8');
const r = pele.render(text, { idPrefix: 'x' });
fs.writeFileSync(process.argv[3], JSON.stringify({ calls, svg: r.svg }));
console.log(process.argv[2], 'layout calls:', calls.length);
