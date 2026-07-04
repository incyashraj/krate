#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "${SCRIPT_DIR}/.." && pwd)"
CRITERION_DIR="${REPO_ROOT}/target/criterion"
OUT_FILE="${REPO_ROOT}/docs/book/src/phase2/benchmark-baseline.json"

DATE_UTC="$(date -u +%F)"
MACHINE_DESC="$(
  {
    cpu="$(sysctl -n machdep.cpu.brand_string 2>/dev/null || true)"
    model="$(sysctl -n hw.model 2>/dev/null || true)"
    os="$(uname -srm 2>/dev/null || true)"
    arch="$(uname -m 2>/dev/null || true)"
    if [ -n "${cpu}" ]; then
      printf '%s, %s' "${cpu}" "${os}"
    elif [ -n "${model}" ]; then
      printf '%s, %s' "${model}" "${os}"
    else
      printf '%s' "${os}"
    fi
  } | sed 's/[[:space:]]\+/ /g'
)"

ruby -rjson -e '
criterion_dir = ARGV.fetch(0)
out_file = ARGV.fetch(1)
recorded_at = ARGV.fetch(2)
machine = ARGV.fetch(3)

metrics = {
  "phase2_component_from_binary_smoke" => "phase2_runtime/component_from_binary_phase2_smoke",
  "phase2_cold_start_to_main_smoke" => "phase2_runtime/cold_start_to_main_phase2_smoke",
  "phase2_loaded_run_smoke" => "phase2_runtime/loaded_run_phase2_smoke",
  "phase2_loaded_run_clock_fixed_time" => "phase2_runtime/loaded_run_krate_clock_fixed_time",
  "phase2_uapi_default_stdout_grant" => "phase2_uapi_dispatch/default_stdout_grant",
  "phase2_uapi_fs_open_read_granted" => "phase2_uapi_dispatch/fs_open_read_granted",
  "phase2_uapi_fs_handle_read_granted" => "phase2_uapi_dispatch/fs_handle_read_granted",
  "phase2_uapi_fs_handle_write_granted" => "phase2_uapi_dispatch/fs_handle_write_granted",
  "phase2_uapi_fs_missing_read_denied" => "phase2_uapi_dispatch/fs_missing_read_denied",
  "phase2_uapi_net_fetch_granted" => "phase2_uapi_dispatch/net_fetch_granted"
}

out_metrics = {}
metrics.each do |name, criterion_path|
  estimates_path = File.join(criterion_dir, criterion_path, "new", "estimates.json")
  unless File.exist?(estimates_path)
    abort("missing benchmark estimate: #{estimates_path}. Run startup + uapi_dispatch benches first.")
  end
  estimates = JSON.parse(File.read(estimates_path))
  out_metrics[name] = {
    "criterion_path" => criterion_path,
    "baseline_ns" => estimates.fetch("mean").fetch("point_estimate").round
  }
end

doc = {
  "recorded_at" => recorded_at,
  "machine" => machine,
  "notes" => "Auto-recorded Phase 2 baseline from local Criterion output; use for regression tracking only.",
  "metrics" => out_metrics
}

File.write(out_file, JSON.pretty_generate(doc) + "\n")
puts "Wrote #{out_file} with #{out_metrics.length} metrics."
' "${CRITERION_DIR}" "${OUT_FILE}" "${DATE_UTC}" "${MACHINE_DESC}"
