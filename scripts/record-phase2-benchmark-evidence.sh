#!/usr/bin/env sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$ROOT"

OUTPUT="target/phase2-benchmark-evidence/benchmark-evidence.md"
STRICT="${KRATE_BENCHMARK_EVIDENCE_STRICT:-0}"
RUN_BENCH="${KRATE_BENCHMARK_EVIDENCE_RUN_BENCH:-1}"
RUN_CLI_STARTUP="${KRATE_BENCHMARK_EVIDENCE_RUN_CLI_STARTUP:-1}"
MODE="${BENCH_REGRESSION_MODE:-warn}"
THRESHOLD_PCT="${BENCH_REGRESSION_THRESHOLD_PCT:-10}"
BASELINE_FILE="${KRATE_BENCHMARK_BASELINE_FILE:-$ROOT/docs/book/src/phase2/benchmark-baseline.json}"

usage() {
  cat <<'USAGE'
Usage: scripts/record-phase2-benchmark-evidence.sh [--strict] [--skip-bench] [--skip-cli-startup] [--mode <warn|fail>] [--threshold <pct>] [--output <path>]

Options:
  --strict            Exit non-zero when any step fails
  --skip-bench        Reuse existing Criterion output instead of re-running benches
  --skip-cli-startup  Skip full external krate CLI startup evidence
  --mode <mode>       Regression mode: warn or fail (default: BENCH_REGRESSION_MODE or warn)
  --threshold <n>     Regression threshold percent (default: BENCH_REGRESSION_THRESHOLD_PCT or 10)
  --output <path>     Output markdown file path

Environment:
  KRATE_BENCHMARK_EVIDENCE_STRICT           1 to exit non-zero when any step fails
  KRATE_BENCHMARK_EVIDENCE_RUN_BENCH        0 to skip benchmark commands
  KRATE_BENCHMARK_EVIDENCE_RUN_CLI_STARTUP  0 to skip full external CLI startup evidence
  KRATE_BENCHMARK_BASELINE_FILE             Baseline JSON path (default: docs/book/src/phase2/benchmark-baseline.json)
USAGE
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --strict)
      STRICT="1"
      shift
      ;;
    --skip-bench)
      RUN_BENCH="0"
      shift
      ;;
    --skip-cli-startup)
      RUN_CLI_STARTUP="0"
      shift
      ;;
    --mode)
      if [ "$#" -lt 2 ]; then
        echo "missing value for --mode" >&2
        usage
        exit 2
      fi
      MODE="$2"
      shift 2
      ;;
    --threshold)
      if [ "$#" -lt 2 ]; then
        echo "missing value for --threshold" >&2
        usage
        exit 2
      fi
      THRESHOLD_PCT="$2"
      shift 2
      ;;
    --output)
      if [ "$#" -lt 2 ]; then
        echo "missing value for --output" >&2
        usage
        exit 2
      fi
      OUTPUT="$2"
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      if [ "$OUTPUT" = "target/phase2-benchmark-evidence/benchmark-evidence.md" ]; then
        OUTPUT="$1"
        shift
      else
        echo "unknown argument: $1" >&2
        usage
        exit 2
      fi
      ;;
  esac
done

mkdir -p "$(dirname "$OUTPUT")"
TMP_DIR="target/phase2-benchmark-evidence/.tmp"
mkdir -p "$TMP_DIR"

STARTUP_LOG="$TMP_DIR/startup.log"
DISPATCH_LOG="$TMP_DIR/dispatch.log"
REGRESSION_LOG="$TMP_DIR/regression.log"
CLI_BUILD_LOG="$TMP_DIR/cli-build.log"
CLOCK_BUILD_LOG="$TMP_DIR/clock-build.log"
CLI_STARTUP_LOG="$TMP_DIR/cli-startup.log"
CLI_STARTUP_REPORT="$TMP_DIR/cli-startup.md"
METRICS_TABLE="$TMP_DIR/metrics-table.md"

if [ "$RUN_BENCH" = "1" ]; then
  if cargo bench -p krate-runtime --bench startup >"$STARTUP_LOG" 2>&1; then
    STARTUP_CODE=0
  else
    STARTUP_CODE=$?
  fi

  if cargo bench -p krate-runtime --bench uapi_dispatch >"$DISPATCH_LOG" 2>&1; then
    DISPATCH_CODE=0
  else
    DISPATCH_CODE=$?
  fi
else
  STARTUP_CODE=0
  DISPATCH_CODE=0
  printf 'benchmark run skipped (--skip-bench)\n' >"$STARTUP_LOG"
  printf 'benchmark run skipped (--skip-bench)\n' >"$DISPATCH_LOG"
fi

if BENCH_BASELINE_FILES="$BASELINE_FILE" BENCH_REGRESSION_MODE="$MODE" BENCH_REGRESSION_THRESHOLD_PCT="$THRESHOLD_PCT" scripts/check-benchmark-regression.sh >"$REGRESSION_LOG" 2>&1; then
  REGRESSION_CODE=0
else
  REGRESSION_CODE=$?
fi

if [ "$RUN_CLI_STARTUP" = "1" ]; then
  if cargo build -p krate-cli --release >"$CLI_BUILD_LOG" 2>&1; then
    CLI_BUILD_CODE=0
  else
    CLI_BUILD_CODE=$?
  fi

  if scripts/build-krate-clock-component.sh >"$CLOCK_BUILD_LOG" 2>&1; then
    CLOCK_BUILD_CODE=0
  else
    CLOCK_BUILD_CODE=$?
  fi

  if [ "$CLI_BUILD_CODE" -eq 0 ] && [ "$CLOCK_BUILD_CODE" -eq 0 ] &&
    cargo run -p krate-tools --bin record-phase2-cli-startup -- \
      --output "$CLI_STARTUP_REPORT" >"$CLI_STARTUP_LOG" 2>&1; then
    CLI_STARTUP_CODE=0
  else
    CLI_STARTUP_CODE=$?
  fi
else
  CLI_BUILD_CODE=0
  CLOCK_BUILD_CODE=0
  CLI_STARTUP_CODE=0
  printf 'CLI startup evidence skipped (--skip-cli-startup)\n' >"$CLI_BUILD_LOG"
  printf 'CLI startup evidence skipped (--skip-cli-startup)\n' >"$CLOCK_BUILD_LOG"
  printf 'CLI startup evidence skipped (--skip-cli-startup)\n' >"$CLI_STARTUP_LOG"
  printf '# Phase 2 CLI Startup Evidence\n\nSkipped.\n' >"$CLI_STARTUP_REPORT"
fi

ruby -rjson -e '
baseline_file = ARGV.fetch(0)
criterion_dir = ARGV.fetch(1)
default_threshold = ARGV.fetch(2)

unless File.exist?(baseline_file)
  warn "missing baseline file: #{baseline_file}"
  exit 2
end

data = JSON.parse(File.read(baseline_file))
metrics = data.fetch("metrics", {})

puts "| Metric | Current ns | Baseline ns | Threshold % |"
puts "|---|---:|---:|---:|"
metrics.each do |name, spec|
  criterion_path = spec.fetch("criterion_path")
  baseline_ns = spec.fetch("baseline_ns")
  threshold = spec.fetch("threshold_pct", default_threshold)
  estimates_path = File.join(criterion_dir, criterion_path, "new", "estimates.json")

  current = if File.exist?(estimates_path)
              estimates = JSON.parse(File.read(estimates_path))
              estimates.fetch("mean").fetch("point_estimate").round
            else
              "n/a"
            end

  puts "| #{name} | #{current} | #{baseline_ns} | #{threshold} |"
end
' "$BASELINE_FILE" "$ROOT/target/criterion" "$THRESHOLD_PCT" >"$METRICS_TABLE"

now_utc="$(date -u +%FT%TZ)"
host_os="$(uname -s 2>/dev/null || printf 'unknown')"
host_arch="$(uname -m 2>/dev/null || printf 'unknown')"
git_commit="$(git rev-parse --short HEAD 2>/dev/null || printf 'unknown')"

result_of() {
  code="$1"
  if [ "$code" -eq 0 ]; then
    printf 'passed'
  else
    printf 'failed'
  fi
}

{
  echo "# Phase 2 Benchmark Evidence"
  echo
  echo "This file is generated by \`scripts/record-phase2-benchmark-evidence.sh\`."
  echo
  echo "## Host"
  echo
  echo "- Git commit: \`$git_commit\`"
  echo "- Host: \`$host_os\` / \`$host_arch\`"
  echo "- Generated at (UTC): \`$now_utc\`"
  echo "- Benchmark run mode: \`$( [ "$RUN_BENCH" = "1" ] && printf 'run' || printf 'reuse' )\`"
  echo "- CLI startup evidence mode: \`$( [ "$RUN_CLI_STARTUP" = "1" ] && printf 'run' || printf 'skipped' )\`"
  echo "- Regression mode: \`$MODE\`"
  echo "- Regression threshold %: \`$THRESHOLD_PCT\`"
  echo "- Baseline file: \`$BASELINE_FILE\`"
  echo
  echo "## Command Results"
  echo
  echo "| Step | Exit code | Result |"
  echo "|---|---:|---|"
  echo "| Startup benchmark (\`cargo bench -p krate-runtime --bench startup\`) | $STARTUP_CODE | $(result_of "$STARTUP_CODE") |"
  echo "| Dispatch benchmark (\`cargo bench -p krate-runtime --bench uapi_dispatch\`) | $DISPATCH_CODE | $(result_of "$DISPATCH_CODE") |"
  echo "| Regression check (\`scripts/check-benchmark-regression.sh\`) | $REGRESSION_CODE | $(result_of "$REGRESSION_CODE") |"
  echo "| CLI release build (\`cargo build -p krate-cli --release\`) | $CLI_BUILD_CODE | $(result_of "$CLI_BUILD_CODE") |"
  echo "| Clock component build (\`scripts/build-krate-clock-component.sh\`) | $CLOCK_BUILD_CODE | $(result_of "$CLOCK_BUILD_CODE") |"
  echo "| Full CLI startup (\`krate run krate-clock\`) | $CLI_STARTUP_CODE | $(result_of "$CLI_STARTUP_CODE") |"
  echo
  echo "## Metric Snapshot"
  echo
  cat "$METRICS_TABLE"
  echo
  echo "## Full CLI Startup Evidence"
  echo
  cat "$CLI_STARTUP_REPORT"
  echo
  echo "## Startup Benchmark Log (tail)"
  echo
  echo '```text'
  tail -n 120 "$STARTUP_LOG"
  echo '```'
  echo
  echo "## Dispatch Benchmark Log (tail)"
  echo
  echo '```text'
  tail -n 120 "$DISPATCH_LOG"
  echo '```'
  echo
  echo "## Regression Check Log (tail)"
  echo
  echo '```text'
  tail -n 120 "$REGRESSION_LOG"
  echo '```'
  echo
  echo "## CLI Build Log (tail)"
  echo
  echo '```text'
  tail -n 120 "$CLI_BUILD_LOG"
  echo '```'
  echo
  echo "## Clock Component Build Log (tail)"
  echo
  echo '```text'
  tail -n 120 "$CLOCK_BUILD_LOG"
  echo '```'
  echo
  echo "## CLI Startup Recorder Log (tail)"
  echo
  echo '```text'
  tail -n 120 "$CLI_STARTUP_LOG"
  echo '```'
} >"$OUTPUT"

echo "wrote $OUTPUT"

if [ "$STRICT" = "1" ] && {
  [ "$STARTUP_CODE" -ne 0 ] ||
  [ "$DISPATCH_CODE" -ne 0 ] ||
  [ "$REGRESSION_CODE" -ne 0 ] ||
  [ "$CLI_BUILD_CODE" -ne 0 ] ||
  [ "$CLOCK_BUILD_CODE" -ne 0 ] ||
  [ "$CLI_STARTUP_CODE" -ne 0 ];
}; then
  exit 1
fi

exit 0
