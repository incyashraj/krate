#!/usr/bin/env bash
# Interaction latencies (PROTOCOL.md 5). Refuses to run, and says why,
# unless Terminal holds Accessibility (to post keys and wheel events) and
# Screen Recording (to see frames). It never asks for or changes either.
#
#   measure-interaction-leg.sh RUN_DIR APP LOAD switcher-search INDEX0
#       one launch: BENCH_INTERACT_RUNS switcher samples (open, result,
#       opening the note) and as many vault searches
#   measure-interaction-leg.sh RUN_DIR APP LOAD typing
#   measure-interaction-leg.sh RUN_DIR APP big-50000 scroll
set -euo pipefail
source "$(dirname "$0")/lib.sh"
load_config

run_dir=${1:?usage: measure-interaction-leg.sh RUN_DIR APP LOAD SCENARIO}
app=${2:?} load=${3:?} scenario=${4:?}
dir="$run_dir/raw/interact"
/bin/mkdir -p "$dir"
if ! "$BUILD/interact" preflight >"$dir/preflight.json" 2>&1; then
  cat "$dir/preflight.json" >&2
  die "the interaction leg needs Accessibility and Screen Recording for Terminal (System Settings > Privacy & Security); grant them yourself, then rerun with BENCH_LEGS=interaction"
fi
tag="interact-$scenario-$load-$app"
launch_keep "$run_dir" "$app" "$load" warm "$tag" 180
json=$LAUNCH_JSON
pid=$(json_get "$json" pid)
cleanup() { if [[ -n "${pid:-}" ]]; then kill_tree "$pid"; fi; }
trap cleanup EXIT INT TERM
record() { python3 "$ROOT/scripts/record.py" interact --run "$run_dir" --app "$app" --load "$load" "$@"; }

if [[ "$(json_get "$json" content_on_screen)" != yes ]]; then
  names=$scenario
  [[ "$scenario" == switcher-search ]] && names="switcher search"
  for s in $names; do record --scenario "$s" --index 0 --json "$json"; done
  exit 0
fi
/bin/sleep 10   # let indexing finish before any latency is taken

case "$scenario" in
  switcher-search)
    for ((i = 1; i <= BENCH_INTERACT_RUNS; i++)); do
      out="$dir/$tag-switcher-$i.json"
      "$BUILD/interact" switcher --pid "$pid" --query "$(target_field "$load" switcher.query)" \
        --expect "$(target_field "$load" switcher.expect)" --phrases "$BUILD/targets/$load.switcher.tsv" \
        >"$out" 2>"$out.stderr" || true
      record --scenario switcher --index "$i" --json "$out"
      [[ "$(json_get "$out" reason)" == focus_lost ]] && die "focus left the app under test; stopped (see $out)"
      /bin/sleep 1.5
      out="$dir/$tag-search-$i.json"
      "$BUILD/interact" search --pid "$pid" --query "$(target_field "$load" search.query)" \
        --expect "$(target_field "$load" search.expect)" >"$out" 2>"$out.stderr" || true
      record --scenario search --index "$i" --json "$out"
      [[ "$(json_get "$out" reason)" == focus_lost ]] && die "focus left the app under test; stopped (see $out)"
      /bin/sleep 1.5
    done
    ;;
  typing)
    for ((i = 1; i <= BENCH_TYPING_RUNS; i++)); do
      out="$dir/$tag-$i.json"
      "$BUILD/interact" typing --pid "$pid" --note "$(target_field "$load" open_name)" --keys "$BENCH_TYPING_KEYS" \
        >"$out" 2>"$out.stderr" || true
      record --scenario typing --index "$i" --json "$out"
      [[ "$(json_get "$out" reason)" == focus_lost ]] && die "focus left the app under test; stopped (see $out)"
      /bin/sleep 2
    done
    ;;
  scroll)
    for ((i = 1; i <= BENCH_SCROLL_RUNS; i++)); do
      out="$dir/$tag-$i.json"
      "$BUILD/start-probe" --attach "$pid" --phrases "$BUILD/targets/$load.open.tsv" --once >"$out.before" 2>/dev/null || true
      "$BUILD/interact" scroll --pid "$pid" --seconds "$BENCH_SCROLL_SECONDS" --hz "$BENCH_SCROLL_HZ" \
        >"$out" 2>"$out.stderr" || true
      /bin/sleep 0.5
      "$BUILD/start-probe" --attach "$pid" --phrases "$BUILD/targets/$load.open.tsv" --once >"$out.after" 2>/dev/null || true
      record --scenario scroll --index "$i" --json "$out" --before "$out.before" --after "$out.after"
      [[ "$(json_get "$out" reason)" == focus_lost ]] && die "focus left the app under test; stopped (see $out)"
      /bin/sleep 2
    done
    ;;
  *) die "scenario must be switcher-search, typing or scroll" ;;
esac
kill_tree "$pid"
pid=
/bin/sleep 2
