#!/usr/bin/env bash
set -uo pipefail
SD=/private/tmp/claude-501/-Users-yashrajpardeshi-Projects-layer6x6/bd546214-2309-4bf7-a543-23c0c1833aaa/scratchpad
eval "$(sed -n '/^check_assert() {/,/^}/p;/^check_one_key() {/,/^}/p' scripts/benchmark-run.sh)"
echo "=== would each recorded pass hold WITHOUT the alternatives operator? ==="
while IFS=$'\t' read -r id tier req asserts; do
  case "$id" in ''|id) continue;; esac
  O="$SD/bench2/outputs/req-$id.out"
  [ -f "$O" ] || continue
  strict_ok=1
  IFS=';' read -ra AS <<< "$asserts"
  for a in "${AS[@]}"; do
    stripped="${a//|[a-z0-9_-]*/}"
    stripped=$(printf '%s' "$a" | sed 's/|[a-z0-9_-]*//g')
    check_assert "$stripped" "$O" || strict_ok=0
  done
  if [ $strict_ok -eq 1 ]; then
    printf "  req %-2s  passes on the original key alone (teaching sufficed)\n" "$id"
  else
    printf "  req %-2s  NEEDED the alternatives operator\n" "$id"
  fi
done < <(awk -F'\t' 'NF>3 && $1 !~ /^#/' evidence/benchmark/corpus.tsv)
