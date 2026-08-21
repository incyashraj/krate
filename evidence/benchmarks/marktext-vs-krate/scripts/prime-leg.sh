#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"
load_config

run_dir=${1:?usage: prime-leg.sh RUN_DIR APP WORKLOAD}
app=${2:?}
workload=${3:?}
fixture="$ROOT/fixtures/notes-${workload}.md"
profile_root="$run_dir/profiles/$app/$workload"
/bin/mkdir -p "$profile_root"

if [[ "$app" == krate ]]; then
  seed_krate_store "$profile_root/home" "$fixture"
  command=(/usr/bin/env HOME="$profile_root/home" "$KRATE_BIN" run "$KRATE_BUNDLE" --auto-grant)
elif [[ "$app" == marktext ]]; then
  command=("$(marktext_binary)" --user-data-dir="$profile_root/user-data" "$fixture")
else
  die "unknown app: $app"
fi

"$ROOT/build/window-probe" --timeout 30 --terminate -- "${command[@]}" \
  >"$run_dir/raw/prime-$app-$workload.json" \
  2>"$run_dir/raw/prime-$app-$workload.stderr"
