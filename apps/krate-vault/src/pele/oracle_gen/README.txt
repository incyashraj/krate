How oracle_extra.json was made (Pele 1.13.0's own build, under node):

  python3 mine.py mine-in.json                 # the hand-written cases
  python3 rand.py rand-in.json 7 400           # random charts (seed, count)
  python3 rand2.py rich-in.json 5 1000         # random charts with rich labels
  python3 mutate.py bases.json mut-in.json 99 5000   # mutated sources
  node gen.js <cases-in.json> <cases-out.json> # Pele's SVG (or error) for each

gen.js reads ~/krate-bench/vault-render/pele/pele.min.js. Pele's docs corpus
is tests/corpus/flowchart-docs.json in the Pele repository. oracle_extra.json
holds the docs corpus, the hand-written cases and a sample of the others; run
any full set with PELE_CASES=<cases-out.json> cargo test pele::oracle -- --ignored

oracle_<kind>.json for class, state, er, requirement, c4 and block:

  node gen-diagrams.js <kind> [mutated-count]   # default 300 mutants

It takes Pele's specs (tests/compat/<kind>), docs corpus, the parser oracle
test's seeds and fragments, plus hand cases for each renderer's config
switches and icons, and the first rendered sources again at narrow widths.
oracle_diagrams.rs runs them.

The chart kinds (pie, quadrant, xychart, sankey, timeline, journey, gantt,
git, mindmap, sequence) each have oracle_<kind>.json, made by

  node gen-kinds.js <kind> [mutated-count]    # 400 mutated inputs by default

from Pele's own parser oracle tests: every diagram in tests/compat/<kind>
specs and tests/corpus/<kind>-docs.json, the tests' seeds, inputs mutated
with the tests' fragments and mutator (seed 7), the first rendered sources
again at narrow widths and other sizes, and hand cases (configs, icons).
Rendered under TZ=UTC with {idPrefix: 'x', now: 2026-01-08T12:00Z}; the
port takes local time to be UTC. pele::oracle_charts checks them.
