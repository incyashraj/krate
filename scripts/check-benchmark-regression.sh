#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "${SCRIPT_DIR}/.." && pwd)"
CRITERION_DIR="${REPO_ROOT}/target/criterion"
BASELINE_FILES="${BENCH_BASELINE_FILES:-${REPO_ROOT}/docs/book/src/phase1/benchmark-baseline.json:${REPO_ROOT}/docs/book/src/phase2/benchmark-baseline.json}"
MODE="${BENCH_REGRESSION_MODE:-warn}"
THRESHOLD_PCT="${BENCH_REGRESSION_THRESHOLD_PCT:-10}"

case "${MODE}" in
  warn|fail) ;;
  *)
    echo "error: BENCH_REGRESSION_MODE must be 'warn' or 'fail' (got '${MODE}')" >&2
    exit 1
    ;;
esac

case "${THRESHOLD_PCT}" in
  ''|*[!0-9.]*)
    echo "error: BENCH_REGRESSION_THRESHOLD_PCT must be numeric (got '${THRESHOLD_PCT}')" >&2
    exit 1
    ;;
esac

ruby -rjson -e '
baseline_files = ARGV.fetch(0).split(":").reject(&:empty?)
criterion_dir = ARGV.fetch(1)
mode = ARGV.fetch(2)
threshold_pct = Float(ARGV.fetch(3))

baseline_metrics = {}
baseline_files.each do |path|
  unless File.exist?(path)
    puts "::warning::baseline file not found: #{path}"
    next
  end

  data = JSON.parse(File.read(path))
  metrics = data.fetch("metrics", {})
  metrics.each do |metric, spec|
    if baseline_metrics.key?(metric)
      puts "::warning::duplicate benchmark metric '#{metric}' from #{path}; replacing previous entry"
    end
    baseline_metrics[metric] = spec
  end
end

if baseline_metrics.empty?
  puts "::warning::no benchmark baseline metrics found"
  exit 0
end

regressions = 0
checked = 0
missing = 0

baseline_metrics.each do |metric, spec|
  file = File.join(criterion_dir, spec.fetch("criterion_path"), "new", "estimates.json")
  unless File.exist?(file)
    puts "::warning::missing Criterion estimate for #{metric}: #{file}"
    missing += 1
    next
  end

  estimates = JSON.parse(File.read(file))
  current = estimates.fetch("mean").fetch("point_estimate")
  baseline_ns = spec.fetch("baseline_ns")
  allowed = baseline_ns * (1.0 + threshold_pct / 100.0)
  checked += 1

  if current > allowed
    regressions += 1
    pct = ((current - baseline_ns) / baseline_ns.to_f * 100).round(1)
    if mode == "fail"
      puts "::error::#{metric} regressed by #{pct}% (current #{current.round} ns, baseline #{baseline_ns.round} ns, allowed #{allowed.round} ns)"
    else
      puts "::warning::#{metric} regressed by #{pct}% (current #{current.round} ns, baseline #{baseline_ns.round} ns, allowed #{allowed.round} ns)"
    end
  else
    puts "#{metric}: #{current.round} ns (baseline #{baseline_ns.round} ns, threshold #{threshold_pct}%)"
  end
end

puts "Benchmark regression summary: checked=#{checked}, regressions=#{regressions}, missing=#{missing}, mode=#{mode}, threshold=#{threshold_pct}%"
exit(mode == "fail" && regressions > 0 ? 1 : 0)
' "${BASELINE_FILES}" "${CRITERION_DIR}" "${MODE}" "${THRESHOLD_PCT}"
