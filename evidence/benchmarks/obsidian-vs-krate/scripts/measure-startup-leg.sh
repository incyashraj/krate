#!/usr/bin/env bash
# One start sample: launch to window, launch to the note's own text, then
# the app is closed. MODE warm reuses the leg's profile; cold makes a new
# profile and vault copy for this sample and removes it afterwards.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
load_config

run_dir=${1:?usage: measure-startup-leg.sh RUN_DIR APP LOAD MODE INDEX [warmup]}
app=${2:?} load=${3:?} mode=${4:?} index=${5:?} kind=${6:-sample}
case "$mode" in warm) label=warm ;; cold) label="cold-$index" ;; *) die "mode must be warm or cold" ;; esac
tag="startup-$mode-$load-$index-$app"
[[ "$kind" == warmup ]] && tag="warmup-$load-$index-$app"

stamp=$(python3 "$ROOT/scripts/obsidian-profile.py" own-stamp)
prepare_launch "$run_dir" "$app" "$load" "$label"
if [[ "$app" == obsidian ]]; then refuse_other_obsidian; fi
/bin/mkdir -p "$run_dir/raw/launch" "$run_dir/raw/logs" "$run_dir/raw/isolation"
json="$run_dir/raw/launch/$tag.json"
"$BUILD/start-probe" --terminate --timeout "$(timeout_for "$load" "$mode")" \
  --phrases "$BUILD/targets/$load.open.tsv" --child-log "$run_dir/raw/logs/$tag.log" \
  -- "${LAUNCH[@]}" >"$json" 2>"$json.stderr" || true
pid=$(json_get "$json" pid)
if [[ -n "$pid" ]]; then kill_tree "$pid"; fi

iso=-
if [[ "$app" == obsidian ]]; then
  iso="$run_dir/raw/isolation/$tag.json"
  python3 "$ROOT/scripts/obsidian-profile.py" check --user-data "$PROFILE/user-data" --own-stamp "$stamp" >"$iso" || true
fi
if [[ "$kind" != warmup ]]; then
  python3 "$ROOT/scripts/record.py" startup --run "$run_dir" --json "$json" --isolation "$iso" \
    --app "$app" --load "$load" --mode "$mode" --index "$index"
fi
if [[ "$mode" == cold ]]; then
  /usr/bin/du -sk "$PROFILE" >"$run_dir/raw/launch/$tag.profile-size.txt" 2>/dev/null || true
  /bin/rm -rf "$PROFILE"
fi
/bin/sleep 2
