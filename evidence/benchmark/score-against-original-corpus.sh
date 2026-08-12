#!/usr/bin/env bash
# Score every recorded run-3 result against the ORIGINAL corpus (7ef2c85),
# before any edit of mine. The strictest possible reading.
set -uo pipefail
SD=/private/tmp/claude-501/-Users-yashrajpardeshi-Projects-layer6x6/bd546214-2309-4bf7-a543-23c0c1833aaa/scratchpad
eval "$(sed -n '/^check_assert() {/,/^}/p;/^check_one_key() {/,/^}/p' scripts/benchmark-run.sh)"
git show 7ef2c85:evidence/benchmark/corpus.tsv > /tmp/corpus_orig.tsv 2>/dev/null
pass=0; tot=0
while IFS=$'\t' read -r id tier req asserts; do
  case "$id" in ''|id) continue;; esac
  O="$SD/bench2/outputs/req-$id.out"
  [ -f "$O" ] || continue
  tot=$((tot+1)); ok=1
  IFS=';' read -ra AS <<< "$asserts"
  for a in "${AS[@]}"; do check_assert "$a" "$O" || ok=0; done
  [ $ok -eq 1 ] && pass=$((pass+1))
done < <(awk -F'\t' 'NF>3 && $1 !~ /^#/' /tmp/corpus_orig.tsv)
echo "against the ORIGINAL unedited corpus: $pass of $tot recorded requests pass"
