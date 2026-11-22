#!/usr/bin/env bash
# The measured session, one command (PROTOCOL.md). After
#   cp config.example.env config.env   (set the paths)
#   ./scripts/prepare.sh
# run ./scripts/run-suite.sh. BENCH_LEGS in config.env picks the legs.
# Everything is kept in runs/<UTC time>/, analysed, sealed and audited.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
load_config

"$ROOT/scripts/verify-inputs.sh"
[[ -f "$BUILD/machine.prepared.tsv" ]] || die "run ./scripts/prepare.sh first"
refuse_other_obsidian
free_kib=$(/bin/df -k / | /usr/bin/awk 'NR==2{print $4}')
((free_kib > 5 * 1024 * 1024)) || die "less than 5 GiB free on /; free space first"

run_id=$(/bin/date -u '+%Y%m%dT%H%M%SZ')
run_dir=${BENCH_RUN_DIR:-"$ROOT/runs/$run_id"}
/bin/mkdir -p "$run_dir/raw" "$run_dir/artifacts" "$run_dir/kit"
/bin/mkdir -p "$(work_root "$run_dir")"

# What this run is held to: the protocol as committed, and the kit.
python3 - "$ROOT" "$run_dir" <<'PY'
import hashlib, json, subprocess, sys
from pathlib import Path
root, run = Path(sys.argv[1]), Path(sys.argv[2])
def git(*a):
    r = subprocess.run(["git", "-C", str(root), *a], capture_output=True, text=True)
    return r.stdout.strip() if r.returncode == 0 else ""
proto = root / "PROTOCOL.md"
dirty = lambda paths: bool(git("status", "--porcelain", "--", *paths))  # noqa: E731
info = {
    "protocol_sha256": hashlib.sha256(proto.read_bytes()).hexdigest(),
    "protocol_commit": git("log", "-1", "--format=%H", "--", "PROTOCOL.md"),
    "protocol_committed_utc": git("log", "-1", "--format=%cI", "--", "PROTOCOL.md"),
    "protocol_dirty": dirty(["PROTOCOL.md"]),
    "kit_commit": git("rev-parse", "HEAD"),
    "kit_dirty": dirty(["scripts", "config.example.env", "PROTOCOL.md"]),
}
(run / "protocol.json").write_text(json.dumps(info, indent=1, sort_keys=True) + "\n")
print(json.dumps(info))
PY
/bin/cp "$ROOT/PROTOCOL.md" "$run_dir/PROTOCOL.md"
/bin/cp -R "$ROOT/scripts" "$ROOT/config.example.env" "$run_dir/kit/"
/usr/bin/find "$run_dir/kit" -name __pycache__ -prune -exec /bin/rm -rf {} +
/bin/cp "$CONFIG" "$run_dir/config.snapshot.env"
/bin/cp "$KRATE_VAULT_BUNDLE" "$run_dir/artifacts/vault.krate"
/bin/mkdir -p "$run_dir/targets" "$run_dir/vaults"
/bin/cp "$BUILD"/targets/*.json "$BUILD"/targets/*.tsv "$run_dir/targets/"
/bin/cp "$BUILD"/vaults/*.manifest.json "$run_dir/vaults/"
/bin/cp "$BUILD/machine.prepared.tsv" "$run_dir/machine.prepared.tsv"
"$ROOT/scripts/capture-machine.sh" "$run_dir/machine.start.tsv"
"$ROOT/scripts/write-input-manifest.sh" "$run_dir/inputs.start.tsv"
note "run $run_dir; legs: $BENCH_LEGS"

has_leg() { [[ " $BENCH_LEGS " == *" $1 "* ]]; }

if has_leg size; then
  note "size"
  "$ROOT/scripts/measure-size.sh" "$run_dir"
fi

if has_leg startup; then
  for load in $BENCH_LOADS; do
    for ((w = 1; w <= BENCH_WARMUPS; w++)); do
      for app in $(abba_order "$w"); do
        note "warm-up $w $load $app"
        "$ROOT/scripts/measure-startup-leg.sh" "$run_dir" "$app" "$load" warm "$w" warmup || note "leg stopped (exit $?): its missing samples fail the audit"
      done
    done
    for ((r = 1; r <= BENCH_WARM_RUNS; r++)); do
      for app in $(abba_order "$r"); do
        note "startup warm $load $r $app"
        "$ROOT/scripts/measure-startup-leg.sh" "$run_dir" "$app" "$load" warm "$r" || note "leg stopped (exit $?): its missing samples fail the audit"
      done
    done
    for ((r = 1; r <= BENCH_COLD_RUNS; r++)); do
      for app in $(abba_order "$r"); do
        note "startup cold $load $r $app"
        "$ROOT/scripts/measure-startup-leg.sh" "$run_dir" "$app" "$load" cold "$r" || note "leg stopped (exit $?): its missing samples fail the audit"
      done
    done
  done
fi

if has_leg resources; then
  for load in $BENCH_LOADS; do
    for ((r = 1; r <= BENCH_RESOURCE_RUNS; r++)); do
      for app in $(abba_order "$r"); do
        note "resources+idle $load $r $app (~$((BENCH_SETTLE_SECONDS + BENCH_IDLE_SECONDS + 10)) s)"
        "$ROOT/scripts/measure-resources-leg.sh" "$run_dir" "$app" "$load" "$r" || note "leg stopped (exit $?): its missing samples fail the audit"
      done
    done
  done
fi

if has_leg network; then
  r=0
  for mode in cold warm; do
    r=$((r + 1))
    for app in $(abba_order "$r"); do
      note "network $mode light $app (${BENCH_NETWORK_SECONDS} s)"
      "$ROOT/scripts/measure-network-leg.sh" "$run_dir" "$app" light "$mode" 1 || note "leg stopped (exit $?): its missing samples fail the audit"
    done
  done
fi

if has_leg interaction; then
  r=0
  for load in $BENCH_INTERACT_LOADS; do
    r=$((r + 1))
    for app in $(abba_order "$r"); do
      note "interaction switcher+search $load $app"
      "$ROOT/scripts/measure-interaction-leg.sh" "$run_dir" "$app" "$load" switcher-search || note "leg stopped (exit $?): its missing samples fail the audit"
    done
  done
  for load in light big-50000; do
    r=$((r + 1))
    for app in $(abba_order "$r"); do
      note "interaction typing $load $app"
      "$ROOT/scripts/measure-interaction-leg.sh" "$run_dir" "$app" "$load" typing || note "leg stopped (exit $?): its missing samples fail the audit"
    done
  done
  r=$((r + 1))
  for app in $(abba_order "$r"); do
    note "interaction scroll big-50000 $app"
    "$ROOT/scripts/measure-interaction-leg.sh" "$run_dir" "$app" big-50000 scroll || note "leg stopped (exit $?): its missing samples fail the audit"
  done
fi

if has_leg growth; then
  for app in $(abba_order 1); do
    note "growth heavy $app (${BENCH_GROWTH_MINUTES} min)"
    "$ROOT/scripts/measure-growth-leg.sh" "$run_dir" "$app" heavy || note "leg stopped (exit $?): its missing samples fail the audit"
  done
fi

if has_leg energy; then
  for load in $BENCH_LOADS; do
    for app in $(abba_order 1); do
      note "energy $load $app"
      "$ROOT/scripts/measure-energy-leg.sh" "$run_dir" "$app" "$load" || note "energy leg skipped: see above"
    done
  done
fi

if has_leg battery; then
  for app in $(abba_order 1); do
    note "battery medium $app (${BENCH_BATTERY_MINUTES} min)"
    "$ROOT/scripts/measure-battery-leg.sh" "$run_dir" "$app" || note "battery leg skipped: see above"
  done
fi

"$ROOT/scripts/capture-machine.sh" "$run_dir/machine.end.tsv"
"$ROOT/scripts/write-input-manifest.sh" "$run_dir/inputs.end.tsv"
# Krate profiles this run made under ~/.krate/profiles (named bench-<hash>).
/usr/bin/sort -u "$(work_root "$run_dir")/krate-profiles.txt" >"$run_dir/raw/krate-profiles.txt" 2>/dev/null || true
python3 "$ROOT/scripts/analyze.py" "$run_dir"
python3 "$ROOT/scripts/seal.py" "$run_dir"
python3 "$ROOT/scripts/audit.py" "$run_dir" | /usr/bin/tee "$run_dir/audit.txt" || true
note "run kept at $run_dir"
note "to free the space afterwards: rm -rf \"$(work_root "$run_dir")\" and, for each name in raw/krate-profiles.txt, ~/.krate/profiles/<name>"
