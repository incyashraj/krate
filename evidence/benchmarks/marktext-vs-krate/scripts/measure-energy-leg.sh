#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"
load_config

run_dir=${1:?usage: measure-energy-leg.sh RUN_DIR APP WORKLOAD}
app=${2:?}
workload=${3:?}

if ! /usr/bin/sudo -n true 2>/dev/null; then
  die "energy measurement needs a current sudo ticket; run 'sudo -v' yourself, then rerun"
fi

root_pid=$("$ROOT/scripts/start-app.sh" "$run_dir" "$app" "$workload" energy)
cleanup() { kill_tree "$root_pid"; }
trap cleanup EXIT INT TERM
/bin/sleep "$BENCH_SETTLE_SECONDS"

output="$run_dir/raw/powermetrics-$app-$workload.plist"
/usr/bin/sudo -n /usr/bin/powermetrics \
  --show-process-energy \
  --sample-rate 1000 \
  --sample-count 30 \
  --format plist \
  --output-file "$output"

printf '%s\t%s\t%s\t%s\t%s\n' "$(ts_utc)" "$app" "$workload" "$root_pid" "$output" \
  >>"$run_dir/raw/energy-index.tsv"
