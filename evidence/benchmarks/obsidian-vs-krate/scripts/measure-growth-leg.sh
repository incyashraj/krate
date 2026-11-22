#!/usr/bin/env bash
# Memory growth: the tree's footprint every 60 s for GROWTH minutes (60),
# idle, heavy vault open. Warm profile.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
load_config

run_dir=${1:?usage: measure-growth-leg.sh RUN_DIR APP [LOAD]}
app=${2:?} load=${3:-heavy}
tag="growth-$load-$app"
dir="$run_dir/raw/growth"
/bin/mkdir -p "$dir"
launch_keep "$run_dir" "$app" "$load" warm "$tag" 180
json=$LAUNCH_JSON
pid=$(json_get "$json" pid)
cleanup() { if [[ -n "${pid:-}" ]]; then kill_tree "$pid"; fi; }
trap cleanup EXIT INT TERM
samples="$dir/$tag.samples.jsonl"
: >"$samples"
if [[ "$(json_get "$json" content_on_screen)" == yes ]]; then
  "$BUILD/tree-sample" --pid "$pid" --also-exe "$(app_exe_prefix "$app")" \
    --interval 60 --count $((BENCH_GROWTH_MINUTES + 1)) >"$samples" 2>"$samples.stderr" || true
fi
kill_tree "$pid"
pid=
python3 "$ROOT/scripts/record.py" growth --run "$run_dir" --launch "$json" --samples "$samples" --app "$app" --load "$load"
/bin/sleep 2
