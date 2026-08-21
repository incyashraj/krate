#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"
load_config

run_dir=${1:?usage: start-app.sh RUN_DIR APP WORKLOAD LABEL}
app=${2:?}
workload=${3:?}
label=${4:?}
fixture="$ROOT/fixtures/notes-${workload}.md"
profile_root="$run_dir/profiles/$label/$app/$workload"
/bin/mkdir -p "$profile_root"

if [[ "$app" == krate ]]; then
  seed_krate_store "$profile_root/home" "$fixture"
  command=(/usr/bin/env HOME="$profile_root/home" "$KRATE_BIN" run "$KRATE_BUNDLE" --auto-grant)
elif [[ "$app" == marktext ]]; then
  command=("$(marktext_binary)" --user-data-dir="$profile_root/user-data" "$fixture")
else
  die "unknown app: $app"
fi

json_file="$run_dir/raw/start-$label-$app-$workload.json"
"$ROOT/build/window-probe" --timeout 30 -- "${command[@]}" >"$json_file" 2>"$json_file.stderr"
python3 - "$json_file" <<'PY'
import json, sys
print(json.load(open(sys.argv[1]))["pid"])
PY
