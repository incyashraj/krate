#!/usr/bin/env bash
# Optional: battery drain over BATTERY minutes (30) with the app open on the
# medium vault, on battery power. Every 5 minutes a 10-key typing pass if
# Accessibility is granted (mode typing), else idle (mode idle). The charge
# is read from the battery's own counters (ioreg AppleSmartBattery).
set -euo pipefail
source "$(dirname "$0")/lib.sh"
load_config

run_dir=${1:?usage: measure-battery-leg.sh RUN_DIR APP}
app=${2:?} load=medium
dir="$run_dir/raw/battery"
/bin/mkdir -p "$dir"
tag="battery-$load-$app"
charge() {
  python3 - "$1" <<'PY'
import json, re, subprocess, sys
io = subprocess.run(["/usr/sbin/ioreg", "-rn", "AppleSmartBattery"], capture_output=True, text=True).stdout
def num(key):
    m = re.search(rf'"{key}" = (\d+)', io)
    return int(m.group(1)) if m else None
batt = subprocess.run(["/usr/bin/pmset", "-g", "batt"], capture_output=True, text=True).stdout
pct = re.search(r"(\d+)%", batt)
out = {"mah": num("AppleRawCurrentCapacity") or num("CurrentCapacity"), "max_mah": num("AppleRawMaxCapacity") or num("MaxCapacity"),
       "pct": int(pct.group(1)) if pct else None, "on_battery": "Battery Power" in batt, "pmset": batt.strip()}
json.dump(out, open(sys.argv[1], "w"))
PY
}
/usr/bin/pmset -g batt | /usr/bin/grep -q "Battery Power" || die "the battery leg runs on battery power only; unplug the adapter first"
mode=idle
"$BUILD/interact" preflight >/dev/null 2>&1 && mode=typing
launch_keep "$run_dir" "$app" "$load" warm "$tag"
json=$LAUNCH_JSON
pid=$(json_get "$json" pid)
cleanup() { if [[ -n "${pid:-}" ]]; then kill_tree "$pid"; fi; }
trap cleanup EXIT INT TERM
charge "$dir/$tag.before.json"
for ((m = 0; m < BENCH_BATTERY_MINUTES; m += 5)); do
  if [[ "$mode" == typing ]]; then
    "$BUILD/interact" typing --pid "$pid" --keys 10 >"$dir/$tag.typing-$m.json" 2>/dev/null || true
  fi
  /bin/sleep 300
done
charge "$dir/$tag.after.json"
kill_tree "$pid"
pid=
python3 "$ROOT/scripts/record.py" battery --run "$run_dir" --launch "$json" --before "$dir/$tag.before.json" \
  --after "$dir/$tag.after.json" --app "$app" --load "$load" --minutes "$BENCH_BATTERY_MINUTES" --mode "$mode"
