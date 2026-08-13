#!/usr/bin/env bash
set -uo pipefail
SD=/private/tmp/claude-501/-Users-yashrajpardeshi-Projects-layer6x6/bd546214-2309-4bf7-a543-23c0c1833aaa/scratchpad
eval "$(sed -n '/^check_assert() {/,/^}/p;/^check_one_key() {/,/^}/p' scripts/benchmark-run.sh)"
OUT=evidence/benchmark/run3/outputs
was=0; now=0; n=0
while IFS=$'\t' read -r id tier req asserts; do
  case "$id" in ''|id) continue;; esac
  [ -f "$OUT/req-$id.out" ] || continue
  [ "$tier" = "refuse" ] && continue
  n=$((n+1)); ok=1
  IFS=';' read -ra AS <<< "$asserts"
  for a in "${AS[@]}"; do check_assert "$a" "$OUT/req-$id.out" || ok=0; done
  prev=$(awk -F'\t' -v i="$id" '$1==i{print $4}' evidence/benchmark/results-run3-2026-08-13.tsv)
  [ "$prev" = "pass" ] && was=$((was+1))
  [ $ok -eq 1 ] && now=$((now+1))
  if [ "$prev" = "fail" ] && [ $ok -eq 1 ]; then echo "  RECOVERED req $id: $(echo "$req" | head -c 42)"; fi
  if [ "$prev" = "pass" ] && [ $ok -eq 0 ]; then echo "  REGRESSED req $id"; fi
done < <(awk -F'\t' 'NF>3 && $1 !~ /^#/' evidence/benchmark/corpus.tsv)
echo "authored: was $was/$n, now $now/$n"
