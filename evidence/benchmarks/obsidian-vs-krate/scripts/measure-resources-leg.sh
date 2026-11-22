#!/usr/bin/env bash
# Resources and idle, one launch (PROTOCOL.md 5): the note on screen, then
# at T+SETTLE (30 s) a footprint snapshot and the start of IDLE (120 s) of
# 1 s tree samples and top; T+120 is read from those samples; footprint
# again at the end. Warm profile.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
load_config

run_dir=${1:?usage: measure-resources-leg.sh RUN_DIR APP LOAD INDEX}
app=${2:?} load=${3:?} index=${4:?}
tag="resources-$load-$index-$app"
dir="$run_dir/raw/resources"
/bin/mkdir -p "$dir" "$run_dir/raw/isolation"
stamp=$(python3 "$ROOT/scripts/obsidian-profile.py" own-stamp)
launched=$(epoch)
launch_keep "$run_dir" "$app" "$load" warm "$tag" "$(timeout_for "$load" warm)"
json=$LAUNCH_JSON
pid=$(json_get "$json" pid)
cleanup() { if [[ -n "${pid:-}" ]]; then kill_tree "$pid"; fi; }
trap cleanup EXIT INT TERM

samples="$dir/$tag.samples.jsonl"
top_out="$dir/$tag.top.txt"
fp30="$dir/$tag.footprint30.json"
fp_end="$dir/$tag.footprint-end.json"
: >"$samples"
if [[ "$(json_get "$json" content_on_screen)" == yes ]]; then
  wait_s=$(python3 -c "import time; print(max(0.0, $launched + $BENCH_SETTLE_SECONDS - time.time()))")
  /bin/sleep "$wait_s"
  pids=()
  while IFS= read -r p; do [[ -n "$p" ]] && pids+=(-p "$p"); done < <(process_tree "$pid")
  /usr/bin/footprint -j "$fp30" "${pids[@]}" >/dev/null 2>&1 || true
  /usr/bin/top -l $((BENCH_IDLE_SECONDS + 1)) -s 1 -stats pid,command,cpu,power,idlew >"$top_out" 2>/dev/null &
  top_pid=$!
  "$BUILD/tree-sample" --pid "$pid" --also-exe "$(app_exe_prefix "$app")" \
    --interval 1 --count $((BENCH_IDLE_SECONDS + 1)) >"$samples" 2>"$samples.stderr" || true
  wait "$top_pid" || true
  pids=()
  while IFS= read -r p; do [[ -n "$p" ]] && pids+=(-p "$p"); done < <(process_tree "$pid")
  if ((${#pids[@]})); then /usr/bin/footprint -j "$fp_end" "${pids[@]}" >/dev/null 2>&1 || true; fi
fi
kill_tree "$pid"
pid=
iso=-
if [[ "$app" == obsidian ]]; then
  iso="$run_dir/raw/isolation/$tag.json"
  python3 "$ROOT/scripts/obsidian-profile.py" check --user-data "$PROFILE/user-data" --own-stamp "$stamp" >"$iso" || true
fi
python3 "$ROOT/scripts/record.py" resources --run "$run_dir" --launch "$json" --samples "$samples" \
  --top "$top_out" --fp30 "$fp30" --fp-end "$fp_end" --isolation "$iso" \
  --app "$app" --load "$load" --index "$index" --launched-at "$launched"
/bin/sleep 2
