#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"
load_config

run_dir=${1:?usage: measure-scroll-leg.sh RUN_DIR APP WORKLOAD}
app=${2:?}
workload=${3:?}
output="$run_dir/raw/scroll.tsv"
ensure_header "$output" 'timestamp_utc\tapp\tworkload_content_lines\troot_pid\tprocess_count\tavg_cpu_percent\tduration_seconds\trequested_hz\tactual_hz\tposted_events\tfixture_sha256\tstatus\treason\tlast_line_before\tlast_line_after\tcontent_moved'

root_pid=$("$ROOT/scripts/start-app.sh" "$run_dir" "$app" "$workload" scroll)
cleanup() { kill_tree "$root_pid"; }
trap cleanup EXIT INT TERM
/bin/sleep 5

# What is on screen before the scroll, so the scroll can be shown to have
# moved the document and not been dropped (IC-822, test 1746).
before_json="$run_dir/raw/scroll-before-$app-$workload.json"
after_json="$run_dir/raw/scroll-after-$app-$workload.json"
"$ROOT/build/content-probe" --pid "$root_pid" --once >"$before_json" 2>/dev/null || true

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

"$ROOT/build/content-probe" --pid "$root_pid" --once >"$after_json" 2>/dev/null || true

process_count=$(process_tree "$root_pid" | /usr/bin/wc -l | /usr/bin/tr -d ' ')
python3 - "$scroll_json" "$cpu_file" "$output" "$app" "$workload" "$root_pid" \
  "$process_count" "$(sha256_file "$ROOT/fixtures/notes-${workload}.md")" "$status" "$reason" "$(ts_utc)" \
  "$before_json" "$after_json" <<'PY'
import csv, json
from pathlib import Path
import sys
scroll_path, cpu_path, output, app, workload, root_pid, process_count, fixture_sha, status, reason, timestamp, before_path, after_path = sys.argv[1:]
def read(path):
    try:
        return json.loads(Path(path).read_text())
    except Exception:
        return {}
before, after = read(before_path), read(after_path)
line_before, line_after = before.get("last_line", 0), after.get("last_line", 0)
# Moved: the document was on screen both times, further down after.
moved = "yes" if before.get("content_on_screen") and after.get("content_on_screen") and line_after > line_before else "no"
if status == "accepted" and moved != "yes":
    status, reason = "rejected", "scroll_did_not_move_the_document"
try:
    scroll = json.loads(Path(scroll_path).read_text())
except Exception:
    scroll = {}
with Path(cpu_path).open(newline="") as handle:
    rows = list(csv.DictReader(handle, delimiter="\t"))
values = [float(row["tree_cpu_percent"]) for row in rows]
avg = sum(values) / len(values) if values else ""
record = [timestamp, app, workload, root_pid, process_count, avg, scroll.get("duration_seconds", ""), scroll.get("requested_hz", ""), scroll.get("actual_hz", ""), scroll.get("posted_events", ""), fixture_sha, status, reason, line_before, line_after, moved]
with Path(output).open("a") as handle:
    handle.write("\t".join(map(str, record)) + "\n")
PY
