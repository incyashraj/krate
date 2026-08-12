#!/usr/bin/env bash
set -uo pipefail
eval "$(sed -n '/^check_assert() {/,/^}/p;/^check_one_key() {/,/^}/p' scripts/benchmark-run.sh)"
OUT=evidence/benchmark/2026-08-12/outputs
was=0; now=0; n=0
while IFS=$'\t' read -r id tier req asserts; do
  [ -f "$OUT/req-$id.out" ] || continue
  case "$id" in \#*|id) continue;; esac
  n=$((n+1))
  ok=1
  IFS=';' read -ra AS <<< "$asserts"
  for a in "${AS[@]}"; do check_assert "$a" "$OUT/req-$id.out" || ok=0; done
  prev=$(awk -F'\t' -v i="$id" '$1==i{print $4}' evidence/benchmark/results-2026-08-12.tsv)
  [ "$prev" = "pass" ] && was=$((was+1))
  [ $ok -eq 1 ] && now=$((now+1))
  if [ "$prev" = "fail" ] && [ $ok -eq 1 ]; then echo "  RECOVERED req $id: $(echo $req | head -c 44)"; fi
done < <(awk -F'\t' 'NF>3 && $1 !~ /^#/' evidence/benchmark/corpus.tsv)
echo "replayed $n requests with archived output: was $was pass, now $now pass"
