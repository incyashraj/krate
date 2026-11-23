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
