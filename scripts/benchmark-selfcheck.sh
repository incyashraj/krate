#!/usr/bin/env bash
# Validate the benchmark harness against apps we already shipped.
#
# The corpus run needs an AI account to author from. When one is not available
# (K-007), this script still answers the question that matters about the
# harness itself: do gates 2-6 actually run, and do they report honestly on
# real bundles? It takes the .krate files in evidence/store, which were built
# and shipped without any reference to this benchmark, and puts each through
# the same gates a freshly-authored app would face.
#
# What it CANNOT tell you: the score. These apps were not authored from the
# corpus requests, so gate 3 (does what was asked) has no asserts to check.
# What it DOES tell you: whether an already-shipped app clears the mechanical
# gates -- imports, paints a frame, stays open -- and what fraction of them
# print machine-readable state at all, which is the number that decides whether
# the stdout contract is a realistic bar or one nothing meets.
#
# Usage: scripts/benchmark-selfcheck.sh [OUT.tsv]

set -uo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
krate_bin="${KRATE_BIN:-$repo_root/target/release/krate}"
out_tsv="${1:-$repo_root/evidence/benchmark/selfcheck-$(date +%Y-%m-%d).tsv}"
work="${TMPDIR:-/tmp}/krate-benchmark-selfcheck"

if [ ! -x "$krate_bin" ]; then
  echo "no krate binary at $krate_bin -- run: cargo build --release -p krate-cli" >&2
  exit 1
fi

mkdir -p "$work" "$(dirname "$out_tsv")"
printf 'app\timports\tpaints\tstays\tstate_keys\tverdict\tnote\n' > "$out_tsv"

echo "krate binary : $krate_bin"
echo "bundles      : $repo_root/evidence/store"
echo "results      : $out_tsv"
echo

total=0; clean=0; with_state=0
for bundle in "$repo_root"/evidence/store/*.krate; do
  [ -f "$bundle" ] || continue
  app="$(basename "$bundle" .krate)"
  total=$(( total + 1 ))
  dir="$work/$app"; rm -rf "$dir"; mkdir -p "$dir"

  imports=no; paints=no; stays=no; keys=0; note="-"

  # Gate 2: the bundle declares only krate:* capabilities and can be inspected
  # without being run.
  if "$krate_bin" run "$bundle" --dump-caps > "$dir/caps.log" 2>&1; then
    imports=yes
  else
    note="$(head -c 120 "$dir/caps.log" | tr '\t\n' '  ')"
  fi

  # Gates 4-6: the self-exercise run. `-- quick` is the house convention for a
  # headless pass that drives the app and exits rather than waiting forever.
  "$krate_bin" run "$bundle" --auto-grant --shoot "$dir/frame.png" -- quick \
    > "$dir/run.log" 2>"$dir/run.err"
  rc=$?
  # 0 is a clean finish, 2 is a clean close-requested. Anything else fell over.
  if [ "$rc" = "0" ] || [ "$rc" = "2" ]; then stays=yes; fi
  if [ -s "$dir/frame.png" ]; then paints=yes; fi

  # Gate 3's raw material: how many "key:value" lines did it print? This is the
  # number that says whether the stdout contract is realistic today.
  keys="$(grep -cE '^[a-z_][a-z0-9_-]*:' "$dir/run.log" 2>/dev/null | tr -d ' ')"
  [ -z "$keys" ] && keys=0
  [ "$keys" -gt 0 ] && with_state=$(( with_state + 1 ))

  if [ "$imports" = yes ] && [ "$paints" = yes ] && [ "$stays" = yes ]; then
    verdict=clean; clean=$(( clean + 1 ))
  else
    verdict=broken
    [ "$note" = "-" ] && note="exit=$rc"
  fi

  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$app" "$imports" "$paints" "$stays" "$keys" "$verdict" "$note" >> "$out_tsv"
  echo "  $app: imports=$imports paints=$paints stays=$stays state_keys=$keys -> $verdict"
done

echo
echo "  bundles checked          $total"
echo "  clear the mechanical gates $clean"
echo "  print machine-readable state $with_state"
echo
echo "wrote $out_tsv"
