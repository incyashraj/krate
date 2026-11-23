// Makes the oracle for one diagram kind: oracle_<kind>.json beside oracle_extra.json.
//
//   node gen-diagrams.js <kind> [mutated-count]      kind: class, state, er, requirement, c4, block
//
// The cases are Pele's own: every diagram source in its ported Mermaid specs
// (tests/compat/<kind>) and docs corpus (tests/corpus/<kind>-docs.json), the
// seeds its parser oracle test adds, and inputs mutated from those with the
// test's own fragments and mutator (seed 7), then hand cases for the config
// switches and the fit-to-width paths. Each is rendered with Pele's build
// under node with {idPrefix: 'x'}, as the Rust oracle renders it. Cases Pele
// reads as some other diagram type are dropped.
process.env.TZ = 'UTC';
global.window = global; global.self = global;
const fs = require('fs');
const path = require('path');
const HOME = process.env.HOME;
const REPO = HOME + '/krate-bench/vault-render/pele-repo';
eval(fs.readFileSync(HOME + '/krate-bench/vault-render/pele/pele.min.js', 'utf8') + ';global.pele=pele;');

const KINDS = {
  class: { dir: 'class', keyword: /classDiagram/ },
  state: { dir: 'state', keyword: /stateDiagram/ },
  er: { dir: 'er', keyword: /erDiagram/ },
  requirement: { dir: 'requirement', keyword: /requirementDiagram/i },
  c4: { dir: 'c4', keyword: /C4(?:Context|Container|Component|Dynamic|Deployment)/ },
  block: { dir: 'block', keyword: /block/ },
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
const count = Number(process.argv[3] ?? 300);
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

// Hand cases: the config switches each renderer reads.
const ICONS = { iconsMap: { 'fa:fa-car': '<path d="M1 1h22"/>', 'fab:fa-github': '<path d="M2 2"/>' } };
const HAND = {
  class: [
    ['%%{init: {"class": {"hierarchicalNamespaces": false}}}%%\nclassDiagram\nnamespace A.B {\n class C\n}\nnamespace A.D {\n class E\n}\nC --> E', {}],
    ['%%{init: {"class": {"hideEmptyMembersBox": true, "nodeSpacing": 80, "rankSpacing": 20}}}%%\nclassDiagram\nclass A\nclass B {\n+x\n}\nA --> B', {}],
    ['classDiagram\ndirection LR\nA "1" --> "*" B : has\nA "1" --> "2" C\nA "0..1" --> "many" D\nA --> E\nB --> E', {}],
    ['classDiagram\nA --> A : self\nA --> A\nA "1" --> "2" A', {}],
    ['classDiagram\nclass A {\n+int x$\n+run()* void\n-List~int~ items\n#get(T a, U b) Map~K,V~\n}\nclassDef big font-size:24px,fill:#000,color:#fff\nclass A:::big', {}],
    ['classDiagram\nclass Car["fa:fa-car Car"]\nnote for Car "fab:fa-github note"\nCar --> Bus : fa:fa-car rides\nlink Car "https://example.com" "tip"\nclick Bus href "https://example.org" _self', ICONS],
    ['classDiagram\nclass A {\n+int x$\n+run()* void\n}\nclassDef big fill:#000,color:#fff\nclass A:::big\nA --> B : uses', {}],
    ['classDiagram\nclass Car["fa:fa-car Car"]\nnote for Car "fab:fa-github note"\nCar --> Bus : rides\nlink Car "https://example.com" "tip"\nclick Bus href "https://example.org" _self', ICONS],
    ['classDiagram\nnamespace Outer {\n class A\n class B\n}\nnamespace Other {\n class C\n}\nA --> C\nB --> C\nC --> A : back\nA <|-- B\nA *-- C\nA o-- B\nA ()-- Lolli', {}],
  ],
  state: [
    ['%%{init: {"flowchart": {"curve": "linear"}, "state": {"nodeSpacing": 70, "rankSpacing": 30}}}%%\nstateDiagram-v2\n[*] --> A\nA --> B : go\nB --> A : back\nB --> [*]', {}],
    ['stateDiagram-v2\nstate Fork <<fork>>\n[*] --> Fork\nFork --> A\nFork --> B\nstate Join <<join>>\nA --> Join\nB --> Join\nJoin --> C\nC --> C : self\nC --> C : again\nnote right of C : a note\nnote left of A\n  two lines\n  of note\nend note\nclick C "https://example.com" "tip"', {}],
    ['stateDiagram-v2\ndirection LR\nstate Comp {\n  direction TB\n  [*] --> X\n  X --> Y\n  --\n  [*] --> P\n  P --> Q\n}\n[*] --> Comp\nComp --> Done\nComp --> X\nstate choice1 <<choice>>\nDone --> choice1\nchoice1 --> Comp : retry\nchoice1 --> [*]', {}],
    ['stateDiagram-v2\nclassDef hot fill:#f00,color:#fff,font-weight:bold\nclassDef cool fill:#00f\nA --> B\nB --> C\nclass A hot\nclass B,C cool\nstyle C stroke:#0f0,stroke-width:3px\nA: fa:fa-car Drive\nB: line one\nB: line two', ICONS],
    ['%%{init: {"flowchart": {"curve": "step"}}}%%\nstateDiagram-v2\nstate "Long name" as L\n[*] --> L\nL --> M : fab:fa-github label\nM --> [*]\nstate M {\n  a --> b\n  b --> a\n}', ICONS],
  ],
  er: [
    ['%%{init: {"er": {"nodeSpacing": 80, "rankSpacing": 30}}}%%\nerDiagram\ndirection LR\nA ||--o{ A : self\nA ||--|{ A : again\nA {\n  int id PK "the key"\n  string name UK, FK\n  List~int~ items\n}\nA }|..|| B : has\nclassDef internal-link fill:#f00\nclass B internal-link', {}],
    ['erDiagram\nsubgraph S [Group one]\ndirection LR\nX ||--o{ Y : owns\nend\nY ||--|| Z : is\nZ }o--o{ X : maps\nstyle X fill:#000,color:#fff\nstyle S fill:#eef', {}],
    ['erDiagram\nsubgraph S [Group alpha]\ndirection LR\nX ||--o{ Y : owns\nend\nY ||--|| Z : is\nZ }o--o{ X : maps\nstyle X fill:#000,color:#fff\nstyle S fill:#eef', {}],
    ['erDiagram\nCAR["fa:fa-car Car"] {\n  string make\n}\nCAR ||--o{ DRIVER : "fab:fa-github drives"\nDRIVER |o--|| CAR : owns\nDRIVER }|--|{ CAR : rents\nDRIVER u--|| CAR : parent', ICONS],
  ],
  requirement: [
    ['requirementDiagram\ndirection LR\nrequirement r {\n  id: 1\n  text: the text\n  risk: high\n  verifymethod: test\n}\nelement e {\n  type: simulation\n  docref: doc/x\n}\ne - satisfies -> r\nr - contains -> r\nr - traces -> e\ne - verifies -> r', {}],
    ['requirementDiagram\nfunctionalRequirement "fa:fa-car Car" {\n  id: 2\n  text: "**bold** text"\n}\nelement "fab:fa-github e" {\n}\n"fab:fa-github e" - refines -> "fa:fa-car Car"\nclassDef hot fill:#000,color:#fff,font-style:italic\nclass "fa:fa-car Car" hot', ICONS],
  ],
  c4: [
    ['C4Dynamic\ntitle Dynamic\nContainer(a, "A", "Tech", "Desc")\nContainer(b, "B")\nContainerDb(c, "C", "SQL")\nRelIndex(1, a, b, "Calls", "HTTP")\nRel(b, c, "Reads")\nRel(c, a, "Notifies")\nUpdateRelStyle(a, b, $textColor="red", $offsetX="40", $offsetY="-20")\nUpdateElementStyle(c, $bgColor="#000", $fontColor="#fff")', {}],
    ['C4Context\nUpdateLayoutConfig($c4ShapeInRow="2", $c4BoundaryInRow="1")\nPerson(u, "User")\nEnterprise_Boundary(e, "Corp") {\n  System(s1, "One")\n  System_Boundary(sb, "Inner") {\n    SystemDb(d, "Store")\n    SystemQueue(q, "Queue")\n  }\n}\nSystem_Ext(x, "Ext")\nRel(u, s1, "Uses")\nBiRel(s1, d, "Syncs")\nRel_Back(q, x, "Feeds")\nRel(u, x, "Calls")\nRel(x, u, "Replies")', {}],
    ['C4Context\nPerson(u, "fa:fa-car User", "fab:fa-github rides")\nSystem_Boundary(b, "fa:fa-car Fleet") {\n  System(s, "Cars")\n}\nRel(u, s, "fa:fa-car drives", "fab:fa-github tech")', ICONS],
    ['C4Deployment\nDeployment_Node(n, "Node", "Ubuntu") {\n  Container(api, "API", "Go", $tags="queue")\n  Node_L(m, "Mobile") {\n    Container(app, "App", $sprite="person")\n  }\n}\nRel(app, api, "Calls", $link="https://example.com")\nRel(api, api, "Self")', {}],
  ],
  block: [
    ['%%{init: {"block": {"padding": 20}}}%%\nblock\ncolumns 3\na b c\nd:2 e\nblock:g\n columns 2\n x y\nend\na --> e\nb -- "lbl" --> d\nc --> c\nd -.-> x', {}],
    ['block-beta\ncolumns 4\nA<["fa:fa-car go"]>(right)\nB<[" "]>(x)\nC<["up"]>(up, down)\nD<["all"]>(left, up)\nE(("round"))\nF{"diamond"}\nG[("db")]\nH{{"hex"}}\nE --> F\nF --> G\nG == "fab:fa-github" ==> H\nH --x E\nE --> E\nF --> F', ICONS],
    ['block\ncolumns auto\nspace:2\nA\nspace\nB:2\nclassDef c fill:#000,color:#fff\nclass A,B c\nstyle B stroke:#f00,stroke-width:4px\nA --> B\nA --> B\nA -- "one" --> B\nA -- "two" --> B', {}],
  ],
};
const sized = [{ maxWidth: 320 }, { maxWidth: 180, fontSize: 12, padding: 4 }, { fontSize: 20, responsive: false }];
const rendered = [];
for (const c of cases) {
  try {
    if (pele.render(c.src, { idPrefix: 'x' }).type === kind) rendered.push(c.src);
  } catch {}
}
for (const src of rendered.slice(0, 12)) {
  sized.forEach((o, k) => cases.push({ name: `${kind}-sized-${cases.length}-${k}`, src, opts: o }));
}
(HAND[kind] ?? []).forEach(([src, o], k) => {
  cases.push({ name: `${kind}-hand-${k}`, src, opts: o });
  cases.push({ name: `${kind}-hand-${k}-narrow`, src, opts: Object.assign({ maxWidth: 260 }, o) });
});

// The options as pele.render takes them: an icon map becomes the host's resolver.
const renderOptions = (opts) => {
  const o = Object.assign({ idPrefix: 'x' }, opts || {});
  if (o.iconsMap) {
    const m = o.iconsMap;
    o.icons = (n) => m[n];
    delete o.iconsMap;
  }
  return o;
};
const out = [];
for (const c of cases) {
  let type = null;
  try {
    type = pele.detectType(c.src);
  } catch {}
  try {
    const r = pele.render(c.src, renderOptions(c.opts));
    if (r.type !== kind) continue;
    out.push(Object.assign({ name: c.name, src: c.src, svg: r.svg }, c.opts ? { opts: c.opts } : {}));
  } catch (e) {
    if (e.type !== undefined && e.type !== kind) continue;
    if (e.type === undefined && type !== null && type !== kind) continue;
    out.push(Object.assign({ name: c.name, src: c.src, error: String(e.message) }, c.opts ? { opts: c.opts } : {}));
  }
}
// Text no type is detected in tells nothing about this kind; keep three of them.
let undetected = 0;
const kept = out.filter((c) => c.error !== 'No diagram type detected for the given text.' || ++undetected <= 3);
const file = path.join(__dirname, '..', `oracle_${kind}.json`);
fs.writeFileSync(file, JSON.stringify(kept, null, 0));
console.log(file, kept.length, 'cases,', kept.filter((c) => c.error).length, 'errors');
