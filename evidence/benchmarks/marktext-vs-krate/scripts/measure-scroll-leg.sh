#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"
load_config

run_dir=${1:?usage: measure-scroll-leg.sh RUN_DIR APP WORKLOAD}
app=${2:?}
workload=${3:?}
output="$run_dir/raw/scroll.tsv"
ensure_header "$output" 'timestamp_utc\tapp\tworkload_content_lines\troot_pid\tprocess_count\tavg_cpu_percent\tduration_seconds\trequested_hz\tactual_hz\tposted_events\tfixture_sha256\tstatus\treason'

root_pid=$("$ROOT/scripts/start-app.sh" "$run_dir" "$app" "$workload" scroll)
cleanup() { kill_tree "$root_pid"; }
trap cleanup EXIT INT TERM
/bin/sleep 5

scroll_json="$run_dir/raw/scroll-driver-$app-$workload.json"
cpu_file="$run_dir/raw/scroll-cpu-$app-$workload.tsv"
printf 'sample\ttimestamp_utc\ttree_cpu_percent\tpids\n' >"$cpu_file"

"$ROOT/build/scroll-driver" "$root_pid" "$BENCH_SCROLL_SECONDS" "$BENCH_SCROLL_HZ" >"$scroll_json" 2>"$scroll_json.stderr" &
driver_pid=$!
sample=0
while /bin/kill -0 "$driver_pid" 2>/dev/null; do
  pids=()
  while IFS= read -r pid; do [[ -n "$pid" ]] && pids+=("$pid"); done < <(process_tree "$root_pid")
  ((${#pids[@]})) || break
  joined=$(IFS=,; printf '%s' "${pids[*]}")
  cpu=$(instant_cpu_tree "$root_pid")
  sample=$((sample + 1))
  printf '%s\t%s\t%s\t%s\n' "$sample" "$(ts_utc)" "$cpu" "$joined" >>"$cpu_file"
done

status=accepted
reason=-
if ! wait "$driver_pid"; then
  status=rejected
  reason=scroll_driver_failed_or_accessibility_denied
fi

process_count=$(process_tree "$root_pid" | /usr/bin/wc -l | /usr/bin/tr -d ' ')
python3 - "$scroll_json" "$cpu_file" "$output" "$app" "$workload" "$root_pid" \
  "$process_count" "$(sha256_file "$ROOT/fixtures/notes-${workload}.md")" "$status" "$reason" "$(ts_utc)" <<'PY'
import csv, json
from pathlib import Path
import sys
scroll_path, cpu_path, output, app, workload, root_pid, process_count, fixture_sha, status, reason, timestamp = sys.argv[1:]
try:
    scroll = json.loads(Path(scroll_path).read_text())
except Exception:
    scroll = {}
with Path(cpu_path).open(newline="") as handle:
    rows = list(csv.DictReader(handle, delimiter="\t"))
values = [float(row["tree_cpu_percent"]) for row in rows]
avg = sum(values) / len(values) if values else ""
record = [timestamp, app, workload, root_pid, process_count, avg, scroll.get("duration_seconds", ""), scroll.get("requested_hz", ""), scroll.get("actual_hz", ""), scroll.get("posted_events", ""), fixture_sha, status, reason]
with Path(output).open("a") as handle:
    handle.write("\t".join(map(str, record)) + "\n")
PY
