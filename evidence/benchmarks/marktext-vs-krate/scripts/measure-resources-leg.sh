#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"
load_config

run_dir=${1:?usage: measure-resources-leg.sh RUN_DIR APP WORKLOAD}
app=${2:?}
workload=${3:?}
output="$run_dir/raw/resources.tsv"
ensure_header "$output" 'timestamp_utc\tapp\tworkload_content_lines\troot_pid\tprocess_count\tfootprint_bytes\tavg_cpu_percent\tcpu_samples\tsettle_seconds\tfixture_sha256\tstatus\treason'

root_pid=$("$ROOT/scripts/start-app.sh" "$run_dir" "$app" "$workload" resources)
cleanup() { kill_tree "$root_pid"; }
trap cleanup EXIT INT TERM
/bin/sleep "$BENCH_SETTLE_SECONDS"

mapfile_compat=()
while IFS= read -r pid; do
  [[ -n "$pid" ]] && mapfile_compat+=("$pid")
done < <(process_tree "$root_pid")
if ((${#mapfile_compat[@]} == 0)); then
  die "$app exited before resource collection"
fi

footprint_json="$run_dir/raw/footprint-$app-$workload.json"
footprint_args=()
for pid in "${mapfile_compat[@]}"; do footprint_args+=(-p "$pid"); done
/usr/bin/footprint -j "$footprint_json" "${footprint_args[@]}" >/dev/null

cpu_file="$run_dir/raw/cpu-$app-$workload.tsv"
printf 'sample\ttimestamp_utc\ttree_cpu_percent\tpids\n' >"$cpu_file"
for ((sample = 1; sample <= BENCH_CPU_SAMPLES; sample++)); do
  pids=()
  while IFS= read -r pid; do [[ -n "$pid" ]] && pids+=("$pid"); done < <(process_tree "$root_pid")
  joined=$(IFS=,; printf '%s' "${pids[*]}")
  cpu=$(instant_cpu_tree "$root_pid")
  printf '%s\t%s\t%s\t%s\n' "$sample" "$(ts_utc)" "$cpu" "$joined" >>"$cpu_file"
  if ((sample < BENCH_CPU_SAMPLES && BENCH_CPU_INTERVAL_SECONDS > 1)); then
    /bin/sleep "$((BENCH_CPU_INTERVAL_SECONDS - 1))"
  fi
done

python3 - "$footprint_json" "$cpu_file" "$output" "$app" "$workload" "$root_pid" \
  "$BENCH_SETTLE_SECONDS" "$(sha256_file "$ROOT/fixtures/notes-${workload}.md")" "$(ts_utc)" <<'PY'
import csv, json
from pathlib import Path
import sys
footprint_path, cpu_path, output, app, workload, root_pid, settle, fixture_sha, timestamp = sys.argv[1:]
footprint = json.loads(Path(footprint_path).read_text())
with Path(cpu_path).open(newline="") as handle:
    rows = list(csv.DictReader(handle, delimiter="\t"))
values = [float(row["tree_cpu_percent"]) for row in rows]
processes = footprint.get("processes", [])
translated = [str(p.get("pid")) for p in processes if p.get("translated")]
status = "accepted" if not translated else "rejected"
reason = "-" if not translated else "Rosetta translated pids=" + ",".join(translated)
record = [timestamp, app, workload, root_pid, len(processes), footprint["total footprint"], sum(values)/len(values), len(values), settle, fixture_sha, status, reason]
with Path(output).open("a") as handle:
    handle.write("\t".join(map(str, record)) + "\n")
PY
