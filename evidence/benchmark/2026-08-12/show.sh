#!/usr/bin/env bash
# Show one request: what was wanted, what was missing, what the app printed.
SD="$(dirname "$0")"
id="$1"
repo=/Users/yashrajpardeshi/Projects/layer6x6
echo "req $id: $(awk -F'\t' -v i="$id" '$1==i {print $3}' $repo/evidence/benchmark/corpus.tsv)"
echo "  wanted:  $(awk -F'\t' -v i="$id" '$1==i {print $4}' $repo/evidence/benchmark/corpus.tsv)"
awk -F'\t' -v i="$id" '$1==i {printf "  result:  %s (%s/%s asserts)  missing: %s\n", $4, $8, $9, $10}' "$SD/results-2026-08-12.tsv"
echo "  printed:"
sed 's/^/    /' "$SD/outputs/req-$id.out" 2>/dev/null || sed 's/^/    /' "$SD/work/req-$id/run.log" 2>/dev/null || echo "    (no output captured)"
