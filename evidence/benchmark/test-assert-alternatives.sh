#!/usr/bin/env bash
set -uo pipefail
# Pull the two functions out of the harness without running it.
eval "$(sed -n '/^check_assert() {/,/^}/p;/^check_one_key() {/,/^}/p' scripts/benchmark-run.sh)"
pass=0; fail=0
t() { # t <desc> <assert> <file> <expect pass|fail>
  if check_assert "$2" "$3"; then got=pass; else got=fail; fi
  if [ "$got" = "$4" ]; then pass=$((pass+1)); printf "  ok   %s\n" "$1"
  else fail=$((fail+1)); printf "  FAIL %s (got %s, want %s)\n" "$1" "$got" "$4"; fi
}
O=/tmp/t_out
printf 'clicks:60\nframes:61\n' > $O.5
printf 'columns:5\nrows:16\n' > $O.21
printf 'round_trip:yes\nencoded:AAA=\n' > $O.30
printf 'logged:12\ndays:31\n' > $O.28
printf 'items:0\n' > $O.zero
printf 'total:$289.93\n' > $O.money
printf 'roundtrip:no\n' > $O.rtno

# The real K-105 cases: alternatives must now hold.
t "count|clicks on clicks:60"        "count|clicks>=1"      $O.5   pass
t "cols|columns on columns:5"        "cols|columns>=2"      $O.21  pass
t "roundtrip|round_trip present"     "roundtrip|round_trip?" $O.30 pass
t "recorded|logged on logged:12"     "recorded|logged>=1"   $O.28  pass

# The bar must NOT move.
t "absent key still fails"           "count|taps>=1"        $O.zero fail
t "zero still fails >=1"             "items>=1"             $O.zero fail
t "currency still non-numeric"       "total>=1"             $O.money fail
t "single key unchanged"             "clicks>=1"            $O.5   pass
t "!= holds when value differs"      "roundtrip|round_trip!=no" $O.30 pass
t "!= fails when value matches"      "roundtrip!=no"        $O.rtno fail
echo "passed $pass, failed $fail"
[ $fail -eq 0 ]
