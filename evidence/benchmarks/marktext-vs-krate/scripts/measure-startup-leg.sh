#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"
load_config

run_dir=${1:?usage: measure-startup-leg.sh RUN_DIR APP WORKLOAD RUN_INDEX MODE}
app=${2:?}
workload=${3:?}
run_index=${4:?}
mode=${5:?}
fixture="$ROOT/fixtures/notes-${workload}.md"
[[ -f "$fixture" ]] || die "fixture not found: $fixture"

output="$run_dir/raw/startup.tsv"
ensure_header "$output" 'timestamp_utc\tapp\tworkload_content_lines\trun_index\tmode\twindow_ms\tpid\tfixture_sha256\tstatus\treason'

profile_root="$run_dir/profiles/$app/$workload"
if [[ "$mode" == fresh_profile ]]; then
  profile_root="$profile_root/run-$run_index"
elif [[ "$mode" != warm ]]; then
  die "mode must be warm or fresh_profile"
fi
/bin/mkdir -p "$profile_root"

if [[ "$app" == krate ]]; then
  seed_krate_store "$profile_root/home" "$fixture"
  command=(/usr/bin/env HOME="$profile_root/home" "$KRATE_BIN" run "$KRATE_BUNDLE" --auto-grant)
elif [[ "$app" == marktext ]]; then
  command=("$(marktext_binary)" --user-data-dir="$profile_root/user-data" "$fixture")
else
  die "unknown app: $app"
fi

json_file="$run_dir/raw/startup-${workload}-${run_index}-${app}.json"
status=accepted
reason=-
if ! "$ROOT/build/window-probe" --timeout 30 --terminate -- "${command[@]}" >"$json_file" 2>"$json_file.stderr"; then
  status=rejected
  reason=window_timeout_or_process_exit
fi

python3 - "$json_file" "$output" "$app" "$workload" "$run_index" "$mode" \
  "$(sha256_file "$fixture")" "$status" "$reason" "$(ts_utc)" <<'PY'
import json
from pathlib import Path
import sys
json_path, output, app, workload, run_index, mode, fixture_sha, status, reason, timestamp = sys.argv[1:]
try:
    data = json.loads(Path(json_path).read_text())
except Exception:
    data = {}
window_ms = data.get("window_ms", "")
pid = data.get("pid", "")
with Path(output).open("a") as handle:
    handle.write("\t".join(map(str, [timestamp, app, workload, run_index, mode, window_ms, pid, fixture_sha, status, reason])) + "\n")
PY
