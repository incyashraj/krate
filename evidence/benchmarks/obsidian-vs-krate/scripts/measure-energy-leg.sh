#!/usr/bin/env bash
# Optional: powermetrics process energy for 30 s after settling. Needs a
# current sudo ticket the person made themselves (sudo -v); never asks.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
load_config

run_dir=${1:?usage: measure-energy-leg.sh RUN_DIR APP LOAD}
app=${2:?} load=${3:?}
/usr/bin/sudo -n true 2>/dev/null || die "the energy leg needs 'sudo -v' run by you first; skipped"
dir="$run_dir/raw/energy"
/bin/mkdir -p "$dir"
tag="energy-$load-$app"
launch_keep "$run_dir" "$app" "$load" warm "$tag"
json=$LAUNCH_JSON
pid=$(json_get "$json" pid)
cleanup() { if [[ -n "${pid:-}" ]]; then kill_tree "$pid"; fi; }
trap cleanup EXIT INT TERM
ensure_header "$dir/index.tsv" 'timestamp_utc\tapp\tload\troot_pid\tpids\tplist\tstatus'
if [[ "$(json_get "$json" content_on_screen)" == yes ]]; then
  /bin/sleep "$BENCH_SETTLE_SECONDS"
  pids=$(process_tree "$pid" | /usr/bin/paste -sd, -)
  /usr/bin/sudo -n /usr/bin/powermetrics --show-process-energy --sample-rate 1000 --sample-count 30 \
    --format plist --output-file "$dir/$tag.plist"
  append_row "$dir/index.tsv" "$(ts_utc)" "$app" "$load" "$pid" "$pids" "raw/energy/$tag.plist" accepted
else
  append_row "$dir/index.tsv" "$(ts_utc)" "$app" "$load" "$pid" - - rejected
fi
