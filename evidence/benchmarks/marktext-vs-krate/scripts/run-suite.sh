#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"
load_config

[[ -x "$ROOT/build/window-probe" && -x "$ROOT/build/scroll-driver" ]] || \
  die "run ./scripts/prepare.sh first"
"$ROOT/scripts/verify-inputs.sh"

run_id=$(/bin/date -u '+%Y%m%dT%H%M%SZ')
run_dir=${BENCH_RUN_DIR:-"$ROOT/runs/$run_id"}
/bin/mkdir -p "$run_dir/raw" "$run_dir/profiles" "$run_dir/artifacts"
/bin/cp "$CONFIG" "$run_dir/config.snapshot.env"
/bin/cp "$KRATE_BUNDLE" "$run_dir/artifacts/mark-replica.krate"
"$ROOT/scripts/capture-machine.sh" "$run_dir/machine.tsv"
"$ROOT/scripts/write-input-manifest.sh" "$run_dir/inputs.tsv"
"$ROOT/scripts/measure-size.sh" "$run_dir"

# ABBA alternation reduces bias from temperature, caches, and test order.
if [[ "$BENCH_START_MODE" == warm ]]; then
  for workload in 5000 50000; do
    for app in krate marktext; do
      printf 'prime warm profile workload=%s app=%s\n' "$workload" "$app"
      "$ROOT/scripts/prime-leg.sh" "$run_dir" "$app" "$workload"
    done
  done
fi
for workload in 5000 50000; do
  for ((run = 1; run <= BENCH_START_RUNS; run++)); do
    phase=$(((run - 1) % 4))
    if ((phase == 0 || phase == 3)); then order=(krate marktext); else order=(marktext krate); fi
    for app in "${order[@]}"; do
      printf 'startup workload=%s run=%s app=%s\n' "$workload" "$run" "$app"
      "$ROOT/scripts/measure-startup-leg.sh" "$run_dir" "$app" "$workload" "$run" "$BENCH_START_MODE"
      /bin/sleep 1
    done
  done
done

for workload in 5000 50000; do
  for app in krate marktext; do
    printf 'resources workload=%s app=%s\n' "$workload" "$app"
    "$ROOT/scripts/measure-resources-leg.sh" "$run_dir" "$app" "$workload"
    printf 'scroll workload=%s app=%s\n' "$workload" "$app"
    "$ROOT/scripts/measure-scroll-leg.sh" "$run_dir" "$app" "$workload" || true
  done
done

if [[ "${BENCH_ENERGY:-0}" == 1 ]]; then
  printf 'timestamp_utc\tapp\tworkload_content_lines\troot_pid\tplist_path\n' >"$run_dir/raw/energy-index.tsv"
  for workload in 5000 50000; do
    for app in krate marktext; do
      "$ROOT/scripts/measure-energy-leg.sh" "$run_dir" "$app" "$workload"
    done
  done
fi

python3 "$ROOT/scripts/analyze.py" "$run_dir"
python3 "$ROOT/scripts/audit.py" "$run_dir" | tee "$run_dir/audit.txt"
printf 'Run retained at %s\n' "$run_dir"
