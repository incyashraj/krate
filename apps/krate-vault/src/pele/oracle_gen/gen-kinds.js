// Makes the oracle for one chart kind: oracle_<kind>.json beside oracle_extra.json.
//
//   node gen-kinds.js <kind> [mutated-count]
//
// The cases are Pele's own: every diagram source in its ported Mermaid specs
// (tests/compat/<dir>) and docs corpus (tests/corpus/<dir>-docs.json), the
// seeds its parser oracle test adds, and inputs mutated from those with the
// test's own fragments and mutator (seed 7). Each is rendered with Pele's
// build under node with {idPrefix: 'x', now: NOW} in UTC, as the Rust oracle
// renders it.
process.env.TZ = 'UTC';
global.window = global; global.self = global;
const fs = require('fs');
const path = require('path');
const HOME = process.env.HOME;
const REPO = HOME + '/krate-bench/vault-render/pele-repo';
eval(fs.readFileSync(HOME + '/krate-bench/vault-render/pele/pele.min.js', 'utf8') + ';global.pele=pele;');

// 2026-01-08 12:00 UTC: inside the corpus gantt's range, so its today line is drawn.
const NOW = Date.UTC(2026, 0, 8, 12);

const KINDS = {
  pie: { dir: 'pie', keyword: /pie/ },
  quadrant: { dir: 'quadrant', keyword: /quadrant/i },
  xychart: { dir: 'xychart', keyword: /xychart/i },
  sankey: { dir: 'sankey', keyword: /sankey/i },
  timeline: { dir: 'timeline', keyword: /timeline/ },
  journey: { dir: 'journey', keyword: /journey/ },
  gantt: { dir: 'gantt', keyword: /gantt/ },
  git: { dir: 'git', keyword: /gitGraph/ },
  mindmap: { dir: 'mindmap', keyword: /mindmap/ },
  sequence: { dir: 'sequence', keyword: /sequenceDiagram/i },
};

// tests/support/corpus.ts loadCorpus.
function loadCorpus(diagram, keyword) {
  const seeds = new Set();
  const dir = path.join(REPO, 'tests/compat', diagram);
  for (const file of fs.readdirSync(dir).filter((f) => /\.spec\.[jt]s$/.test(f))) {
    const src = fs.readFileSync(path.join(dir, file), 'utf8');
    for (const m of src.matchAll(/`((?:[^`\\]|\\.)*)`|'((?:[^'\\\n]|\\.)*)'|"((?:[^"\\\n]|\\.)*)"/g)) {
      const raw = m[1] ?? m[2] ?? m[3];
      if (!keyword.test(raw)) continue;
      try {
        seeds.add(new Function('return `' + raw.replace(/`/g, '\\`').replace(/\$\{/g, '\\${') + '`')());
      } catch {
        continue;
      }
    }
  }
  for (const sample of JSON.parse(fs.readFileSync(path.join(REPO, 'tests/corpus', `${diagram}-docs.json`), 'utf8'))) {
    seeds.add(sample);
  }
  return [...seeds];
}

function random(seed) {
  let state = seed >>> 0;
  return () => {
    state = (Math.imul(state, 1664525) + 1013904223) >>> 0;
    return state / 4294967296;
  };
}

function mutator(corpus, fragments, rnd) {
  const pick = (list) => list[Math.floor(rnd() * list.length)];
  return () => {
    let s = pick(corpus);
    const edits = 1 + Math.floor(rnd() * 3);
    for (let k = 0; k < edits; k++) {
      const pos = Math.floor(rnd() * (s.length + 1));
      const r = rnd();
      if (r < 0.35) s = s.slice(0, pos) + pick(fragments) + s.slice(pos);
      else if (r < 0.6) s = s.slice(0, pos) + s.slice(pos + 1 + Math.floor(rnd() * 3));
      else if (r < 0.75) {
        const other = pick(corpus);
        const from = Math.floor(rnd() * other.length);
        s = s.slice(0, pos) + other.slice(from, from + Math.floor(rnd() * 20)) + s.slice(pos);
      } else if (r < 0.9) s = s.slice(0, pos) + s.slice(pos, pos + Math.floor(rnd() * 6)) + s.slice(pos);
      else s = s.slice(0, pos) + pick(fragments) + s.slice(pos + 1);
    }
    return s;
  };
}

// The array literal that starts at `const NAME = [` in the parser oracle test, evaluated.
function arrayIn(test, name, scope) {
  const at = test.indexOf(`const ${name} = [`);
  if (at < 0) return [];
  let depth = 0;
  let i = test.indexOf('[', at);
  const start = i;
  let quote = null;
  for (; i < test.length; i++) {
    const c = test[i];
    if (quote) {
      if (c === '\\') i++;
      else if (c === quote) quote = null;
    } else if (c === '"' || c === "'" || c === '`') quote = c;
    else if (c === '[') depth++;
    else if (c === ']' && --depth === 0) break;
  }
  const body = test.slice(start, i + 1).replace(/\.\.\.loadCorpus\([^)]*\),?/g, '');
  try {
    return new Function(...Object.keys(scope), 'return ' + body)(...Object.values(scope));
  } catch {
    return [];
  }
}

function constIn(test, name) {
  const m = new RegExp(`const ${name} = (.*);`).exec(test);
  try {
    return m ? new Function('return ' + m[1])() : undefined;
  } catch {
    return undefined;
  }
}

const kind = process.argv[2];
const count = Number(process.argv[3] ?? 400);
const spec = KINDS[kind];
const test = fs.readFileSync(path.join(REPO, 'tests', spec.dir, 'oracle.test.ts'), 'utf8');
const scope = { BROKEN: constIn(test, 'BROKEN') ?? '' };
const fragments = arrayIn(test, 'FRAGMENTS', scope);
const corpus = [
  ...new Set([...loadCorpus(spec.dir, spec.keyword), ...arrayIn(test, 'corpus', scope), ...arrayIn(test, 'SEEDS', scope)]),
];
const cases = corpus.map((src, i) => ({ name: `${kind}-corpus-${i}`, src }));
const next = mutator(corpus, fragments, random(7));
for (let i = 0; i < count; i++) {
  const src = next();
  if (src.length <= 2000) cases.push({ name: `${kind}-mut-${i}`, src });
}

// Hand cases: configs from Pele's render tests, and the first rendered
// sources again at narrow widths and other sizes, for the fit-to-width paths.
const ICONS = { iconsMap: { 'fa:fa-car': '<path d="M1 1"/>', 'fab:fa-github': '<circle r="2"/>' } };
const HAND = {
  pie: [
    ['%%{init: {"pie": {"legendPosition": "bottom"}}}%%\npie title T\n"a":1\n"b":2\n"c":0.1', {}],
    ['%%{init: {"pie": {"legendPosition": "top", "donutHole": 0.5}}}%%\npie showData\n"a":1\n"b":2', {}],
    ['%%{init: {"pie": {"legendPosition": "left", "textPosition": 0.3}}}%%\npie\n"a":10\n"b":0.2\n"c":0.3\n"d":0.25', {}],
    ['%%{init: {"pie": {"legendPosition": "center", "donutHole": 0.8}}}%%\npie\n"only":5', {}],
    ['%%{init: {"themeVariables": {"pie1": "#000", "pie2": "#ff0"}}}%%\npie\n"a":1\n"b":1', {}],
    ['pie\n"zero":0\n"z2":0', {}],
  ],
  quadrant: [
    ['%%{init: {"quadrantChart": {"chartWidth": 300, "chartHeight": 200, "xAxisPosition": "bottom", "yAxisPosition": "right", "pointRadius": 9}}}%%\nquadrantChart\n title T\n x-axis Low --> High\n y-axis Down --> Up\n quadrant-1 a\n quadrant-2 b\n quadrant-3 c\n quadrant-4 d', {}],
    ['quadrantChart\n x-axis L\n y-axis B\n A: [0.1, 0.1]\n B: [0.12, 0.12]\n C: [0.11, 0.1] radius: 12, color: #ff0000, stroke-color: #00f, stroke-width: 4px\n D:::k: [0.9, 0.9]\n classDef k color: #0f0, stroke-width: 3px', {}],
  ],
  xychart: [
    ['%%{init: {"xyChart": {"showDataLabel": true}}}%%\nxychart horizontal\n title "Sales"\n x-axis [jan, feb, mar]\n y-axis "Revenue" 0 --> 100\n bar [10, 50, 120]\n line [5, -20, 60]', {}],
    ['%%{init: {"xyChart": {"showDataLabel": true, "width": 300, "height": 200, "xAxis": {"showTick": false}, "yAxis": {"showTitle": false}}}}%%\nxychart\n x-axis 0 --> 10\n bar a [1, -2, 3]\n bar b [3, 2, 1]\n line c [1, 2 "two", 3]', {}],
    ['xychart\n line [1.5, 2.25, 1e3]\n line [0.001, 0.002]', {}],
    ['%%{init: {"themeVariables": {"xyChart": {"plotColorPalette": "#111, #222, rgb(1,2,3)"}}}}%%\nxychart\n bar [1,2]\n line [2,1]', {}],
  ],
  sankey: [
    ['%%{init: {"sankey": {"nodeAlignment": "left", "linkColor": "source", "labelStyle": "outlined", "prefix": "$", "suffix": " kWh"}}}%%\nsankey\nA,B,10\nA,C,5\nB,D,7\nC,D,3\nD,E,10\nX,E,0.5', {}],
    ['%%{init: {"sankey": {"nodeAlignment": "right", "linkColor": "target", "nodeColors": {"A": "#f00", "B": "rgb(0,0,255)"}}}}%%\nsankey\nA,B,10\nA,C,5\nB,D,7\nQ,R,1', {}],
    ['%%{init: {"sankey": {"nodeAlignment": "center", "linkColor": "#abcdef", "showValues": false, "width": 300, "height": 200, "nodeWidth": 30, "nodePadding": 2}}}%%\nsankey\nA,B,10\nA,C,5\nB,D,7\nZ,D,1\nD,E,1', {}],
    ['sankey\nA,B,1\nB,C,1\nC,A,1', {}],
    ['sankey\n"a, ""quoted""",b,1e2\nb,c,0\nc,d,-3', {}],
    ['sankey\n' + Array.from({ length: 14 }, (_, i) => `S${i % 3},T${i},${i + 1}`).join('\n'), {}],
    ['sankey\nA,B,1\nA,C,2', { idPrefix: '' }],
  ],
  git: [
    ['%%{init: {"gitGraph": {"rotateCommitLabel": false, "mainBranchName": "trunk", "mainBranchOrder": 2}}}%%\ngitGraph\n commit id: "a" tag: "v1" tag: "release candidate"\n branch dev order: 1\n commit type: HIGHLIGHT\n commit type: REVERSE msg: "oops"\n checkout trunk\n merge dev id: "m1" type: REVERSE tag: "t"\n branch feat\n commit\n checkout dev\n commit id: "x"\n checkout feat\n cherry-pick id: "x"\n checkout trunk\n merge feat', {}],
    ['%%{init: {"gitGraph": {"parallelCommits": true, "showCommitLabel": false}}}%%\ngitGraph TB:\n commit\n branch a\n commit tag: "t1"\n checkout main\n commit\n branch b\n commit\n commit\n checkout a\n commit\n checkout main\n merge a\n merge b', {}],
    ['%%{init: {"gitGraph": {"showBranches": false}, "themeVariables": {"git0": "#ff0000", "git1": "#000"}}}%%\ngitGraph BT:\n commit tag: "x"\n branch hotfix\n commit\n checkout main\n merge hotfix tag: "merged"\n commit', {}],
  ],
  mindmap: [
    ['%%{init: {"mindmap": {"maxNodeWidth": 90}, "themeVariables": {"cScale1": "#123456"}}}%%\nmindmap\n  root((The root of a long tree))\n    A[Square **bold** text that wraps]\n      ::icon(fa:fa-car)\n      A1\n      A2)cloud(\n    B(rounded)\n    :::urgent large\n      B1))bang((\n      B2{{hex}}\n    C\n    D\n    E\n      E1\n        E11', {}],
    ['mindmap\n root\n  ::icon(fa:fa-car)\n  a\n   ::icon(fa:fa-car)\n   b\n  c[fa:fa-car text]', ICONS],
    ['mindmap\n  r[Rect root]\n    x\n    y\n      z', { maxWidth: 200 }],
  ],
  sequence: [
    ['%%{init: {"sequence": {"mirrorActors": false, "rightAngles": true, "messageAlign": "left", "showSequenceNumbers": true}}}%%\nsequenceDiagram\n  box rgb(200, 220, 255) Front end\n    participant A as Alice\n    actor B@{ "type": "database" }\n  end\n  box Aqua Back\n    participant C@{ "type": "queue", "alias": "Queue" }\n  end\n  autonumber 10 5\n  A->>+B: hi there, a long message that wraps maybe\n  B-->>-A: ok\n  A->>A: self call\n  rect rgba(255, 0, 0, 0.2)\n    B-)C: async\n    C--xB: fail\n  end\n  A()->>()C: central\n  A->>()C: half central\n  A<<->>B: both\n  A-|\\B: half\n  B/|-A: reverse\n  properties A: {"class": "important"}\n  link A: Dashboard @ https://example.com\n  links B: {"Repo": "https://example.com"}', {}],
    ['%%{init: {"sequence": {"hideUnusedParticipants": true, "messageMargin": 50, "noteMargin": 20, "boxMargin": 4, "actorMargin": 10}}}%%\nsequenceDiagram\n  participant Z\n  participant A\n  create participant B\n  A->>B: make\n  alt good\n    A->>B: yes\n  else bad\n    B->>A: no\n  else\n    Note over A,B: neither\n  end\n  par one\n    A->>B: x\n  and two\n    B->>A: y\n  end\n  critical c\n    A->>B: z\n  option o\n    Note left of A: left\n  end\n  break stop\n    Note right of B: right\n  end\n  opt maybe\n    loop again\n      A->>B: loop\n    end\n  end\n  destroy B\n  A->>B: bye', {}],
    ['sequenceDiagram\n  participant A as fa:fa-car Alice\n  A->>A: fa:fa-car hi\n  Note over A: fa:fa-car note', ICONS],
    ['sequenceDiagram\n  A->>B: monospace message here\n  B-->>A: and back', { monospace: ['messages'] }],
    ['%%{init: {"wrap": true}}%%\nsequenceDiagram\n  title Wrapped\n  A->>B: a very long message that should wrap at the configured width of the participants\n  Note over A,B: a long note over both participants that wraps across the lifelines', { maxWidth: 300 }],
  ],
  gantt: [
    ['gantt\n dateFormat YYYY-MM-DD\n excludes weekends, 2026-01-14\n weekend friday\n section A\n One :a1, 2026-01-02, 10d\n Two :after a1, 5d\n Three :until a1\n section B\n Mile :milestone, m1, 2026-01-09, 0d\n Mark :vert, v1, 2026-01-07, 0d\n Cr :crit, active, 2026-01-03, 3d\n Dn :done, crit, 2026-01-04, 1d', {}],
    ['%%{init: {"gantt": {"displayMode": "compact", "topAxis": true, "barHeight": 30, "barGap": 2, "topPadding": 80, "gridLineStartPadding": 40, "numberSectionStyles": 3}}}%%\ngantt\n dateFormat YYYY-MM-DD\n axisFormat %a %d %b\n tickInterval 2day\n todayMarker stroke-width:5px,stroke:#0f0\n section S\n a :2026-01-01, 3d\n b :2026-01-02, 3d\n c :2026-01-05, 2d\n section T\n d :2026-01-03, 4d', {}],
    ['gantt\n dateFormat HH:mm\n axisFormat %H:%M\n a :08:00, 30m\n b :after task1, 2h\n c :09:15, 10:45', {}],
    ['gantt\n dateFormat X\n axisFormat %s\n a :1700000000, 1h\n b :1700003600, 45m', {}],
    ['gantt\n dateFormat x\n a :1700000000000, 1000ms', {}],
    ['gantt\n dateFormat Do MMM YYYY\n axisFormat %U %W %V %j %G %g %u %w %y %Y %e %-d %_m %Q %L %f %p %I %Z %%\n tickInterval 1week\n weekday monday\n a :1st Jan 2026, 2w\n b :22nd Feb 2026, 1M', {}],
    ['gantt\n dateFormat MMMM D, YYYY h:mm a\n a :March 3, 2026 1:05 pm, 3h\n b :12:00 am, 1d', {}],
    ['gantt\n dateFormat YYYY-MM-DDTHH:mm:ssZ\n a :2026-01-01T10:00:00+02:00, 1d\n b :2026-01-02T00:00:00Z, 12h', {}],
    ['gantt\n dateFormat YYYY-MM-DD\n inclusiveEndDates\n includes 2026-01-10\n excludes saturday, sunday\n a :2026-01-05, 2026-01-12\n b :2026-01-05, 8d\n click a href "https://example.com"\n click b call cb(1, 2)', {}],
    ['gantt\n dateFormat YYYY\n axisFormat %Y\n a :2001, 20y\n b :2010, 3y', {}],
    ['gantt\n dateFormat YYYY-MM-DD\n a :2026-01-01, 1d\n b :Jan 5 2026, 2d\n c :2026/02/03, 1d', {}],
    ['gantt\n title Only a title', {}],
    ['gantt\n dateFormat YYYY-MM-DD\n a :2026-01-07, 3d\n b :2026-01-09, 1d', { now: Date.UTC(2026, 0, 8, 15, 30) }],
  ],
};
// Labels with icons, drawn with a host resolver: Pele hands it to some labels and not others.
const ICON_SOURCES = {
  pie: 'pie title fa:fa-car Cars\n"fa:fa-car car" : 3\n"b fab:fa-github" : 2',
  quadrant: 'quadrantChart\n title fa:fa-car T\n x-axis fa:fa-car L --> R\n quadrant-1 fa:fa-car q\n "fa:fa-car p": [0.3, 0.4]',
  xychart: 'xychart\n title "fa:fa-car T"\n x-axis [fa:fa-car, b]\n bar "fa:fa-car s" [1, 2]\n line "l" [2 "fa:fa-car x", 1]',
  sankey: 'sankey\nfa:fa-car A,B,1',
  timeline: 'timeline\n title fa:fa-car T\n section fa:fa-car S\n fa:fa-car p : fa:fa-car e',
  journey: 'journey\n title fa:fa-car T\n section fa:fa-car S\n fa:fa-car t: 5: fa:fa-car me',
  gantt: 'gantt\n title fa:fa-car T\n dateFormat YYYY-MM-DD\n section fa:fa-car S\n fa:fa-car t :a, 2026-01-01, 3d',
};
if (ICON_SOURCES[kind]) {
  HAND[kind] = (HAND[kind] ?? []).concat([[ICON_SOURCES[kind], ICONS], [ICON_SOURCES[kind], {}]]);
}
const sized = [{ maxWidth: 320 }, { maxWidth: 180, fontSize: 12, padding: 4 }, { fontSize: 20, responsive: false }];
const rendered = [];
for (const c of cases) {
  try {
    pele.render(c.src, { idPrefix: 'x', now: NOW });
    rendered.push(c.src);
  } catch {}
}
for (const src of rendered.slice(0, 8)) {
  sized.forEach((o, k) => cases.push({ name: `${kind}-sized-${cases.length}-${k}`, src, opts: o }));
}
(HAND[kind] ?? []).forEach(([src, o], k) => {
  cases.push({ name: `${kind}-hand-${k}`, src, opts: o });
  cases.push({ name: `${kind}-hand-${k}-narrow`, src, opts: Object.assign({ maxWidth: 260 }, o) });
});

const out = [];
for (const c of cases) {
  const opts = Object.assign({ now: NOW }, c.opts || {});
  try {
    const o = Object.assign({ idPrefix: 'x' }, opts);
    if (o.iconsMap) {
      const m = o.iconsMap;
      o.icons = (n) => m[n];
      delete o.iconsMap;
    }
    const r = pele.render(c.src, o);
    out.push({ name: c.name, src: c.src, svg: r.svg, opts });
  } catch (e) {
    out.push({ name: c.name, src: c.src, error: String(e.message), opts });
  }
}
// Text no type is detected in tells nothing about this kind; keep three of them.
let undetected = 0;
const kept = out.filter((c) => c.error !== 'No diagram type detected for the given text.' || ++undetected <= 3);
out.length = 0;
out.push(...kept);
const file = path.join(__dirname, '..', `oracle_${kind}.json`);
fs.writeFileSync(file, JSON.stringify(out, null, 0));
console.log(file, out.length, 'cases,', out.filter((c) => c.error).length, 'errors');
