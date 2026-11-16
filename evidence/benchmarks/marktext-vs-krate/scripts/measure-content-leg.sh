#!/usr/bin/env bash
# Is the document on screen, not only a window (IC-822, test 1745)? Starts
# the app on the workload, waits for the document's own lines to be read
# off its window, and records the answer. A run whose app never shows the
# document has its performance values left out of the analysis.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
load_config

run_dir=${1:?usage: measure-content-leg.sh RUN_DIR APP WORKLOAD}
app=${2:?}
workload=${3:?}
output="$run_dir/raw/content.tsv"
ensure_header "$output" 'timestamp_utc\tapp\tworkload_content_lines\tcontent_on_screen\tcontent_ms\tlines_read\tfirst_line\tlast_line\tfixture_sha256\tstatus\treason'

root_pid=$("$ROOT/scripts/start-app.sh" "$run_dir" "$app" "$workload" content)
cleanup() { kill_tree "$root_pid"; }
trap cleanup EXIT INT TERM

json_file="$run_dir/raw/content-$app-$workload.json"
status=accepted
reason=-
if ! "$ROOT/build/content-probe" --pid "$root_pid" --timeout 30 >"$json_file" 2>"$json_file.stderr"; then
  status=rejected
  reason=document_not_on_screen
fi

python3 - "$json_file" "$output" "$app" "$workload" "$(sha256_file "$ROOT/fixtures/notes-${workload}.md")" "$status" "$reason" "$(ts_utc)" <<'PY'
import json, sys
from pathlib import Path
json_path, output, app, workload, fixture_sha, status, reason, timestamp = sys.argv[1:]
try:
    data = json.loads(Path(json_path).read_text())
except Exception:
    data = {}
if data.get("problem"):
    reason = data["problem"].replace("\t", " ")
on = "yes" if data.get("content_on_screen") else "no"
row = [timestamp, app, workload, on, data.get("content_ms", ""), data.get("lines_read", 0),
       data.get("first_line", 0), data.get("last_line", 0), fixture_sha, status, reason]
with Path(output).open("a") as handle:
    handle.write("\t".join(map(str, row)) + "\n")
PY
