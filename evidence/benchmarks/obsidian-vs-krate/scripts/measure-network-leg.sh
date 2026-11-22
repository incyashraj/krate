#!/usr/bin/env bash
# Network on start: every remote address the app's processes hold in the
# first NETWORK seconds (60), polled from just before launch. MODE cold
# uses a new profile; warm the leg's warm profile.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
load_config

run_dir=${1:?usage: measure-network-leg.sh RUN_DIR APP LOAD MODE INDEX}
app=${2:?} load=${3:?} mode=${4:?} index=${5:?}
label=warm
[[ "$mode" == cold ]] && label="netcold-$index"
tag="network-$mode-$load-$index-$app"
dir="$run_dir/raw/network"
/bin/mkdir -p "$dir"
polls="$dir/$tag.polls.jsonl"
rootfile="$dir/$tag.rootpid"
: >"$rootfile"
python3 "$ROOT/scripts/net-poll.py" --seconds "$BENCH_NETWORK_SECONDS" --interval 0.5 \
  --exe-prefix "$(app_exe_prefix "$app")" --root-file "$rootfile" >"$polls" 2>"$polls.stderr" &
poller=$!
launch_keep "$run_dir" "$app" "$load" "$label" "$tag" "$(timeout_for "$load" "$mode")"
json=$LAUNCH_JSON
pid=$(json_get "$json" pid)
printf '%s\n' "$pid" >"$rootfile"
wait "$poller" || true
if [[ -n "$pid" ]]; then kill_tree "$pid"; fi
python3 "$ROOT/scripts/record.py" network --run "$run_dir" --launch "$json" --polls "$polls" \
  --app "$app" --load "$load" --mode "$mode" --index "$index"
if [[ "$mode" == cold ]]; then /bin/rm -rf "$PROFILE"; fi
/bin/sleep 2
