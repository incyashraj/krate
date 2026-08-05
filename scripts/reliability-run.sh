#!/usr/bin/env bash
# Measure how often AI authoring actually works.
#
# Runs requests from evidence/reliability/corpus.txt one at a time through
# `krate create --agent claude`, and records for each: pass or fail, the stage
# it failed at (from check-app's exit codes), how long it took, how big the
# .krate came out, and how many lines of Rust the agent wrote.
#
# It is resumable. Every finished request is appended to the results file as
# one TSV row keyed by its corpus line number, and a re-run skips any number
# already present. A full 100-request run takes many hours, so stopping and
# resuming is the normal way to use this, not the exception.
#
# Requests run one after another, never in parallel: each one spawns a Claude
# Code session, and several at once thrash the machine and each other's builds.
#
# Usage:
#   scripts/reliability-run.sh                 # every request in the corpus
#   scripts/reliability-run.sh --count 25      # the first 25 not yet done
#   scripts/reliability-run.sh --from 40 --count 10
#   scripts/reliability-run.sh --only 3,7,88   # exactly these corpus numbers
#   scripts/reliability-run.sh --summary       # print the table, run nothing
#
# Do not rebuild the krate binary while a run is in progress. The harness
# invokes $KRATE_BIN for every request, so a rebuild half way through means the
# first half of the results measured one build and the second half measured
# another, and the pass rate is then a number about nothing. Finish the run, or
# copy the binary somewhere and point KRATE_BIN at the copy.
#
# Environment:
#   KRATE_BIN     the krate binary to test (default: target/release/krate)
#   RESULTS       the TSV to append to (default: evidence/reliability/results-<date>.tsv)
#   WORK_ROOT     where per-request work dirs live (default: a tmp dir kept for inspection)
#   TIMEOUT_SECS  per-request budget handed to the agent (default: 900)

set -uo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
corpus="$repo_root/evidence/reliability/corpus.txt"
krate_bin="${KRATE_BIN:-$repo_root/target/release/krate}"
results="${RESULTS:-$repo_root/evidence/reliability/results-$(date +%Y-%m-%d).tsv}"
work_root="${WORK_ROOT:-${TMPDIR:-/tmp}/krate-reliability}"
timeout_secs="${TIMEOUT_SECS:-900}"

count=0
from=1
only=""
summary_only=0

while [ $# -gt 0 ]; do
  case "$1" in
    --count) count="$2"; shift 2 ;;
    --from) from="$2"; shift 2 ;;
    --only) only="$2"; shift 2 ;;
    --summary) summary_only=1; shift ;;
    --results) results="$2"; shift 2 ;;
    -h|--help) sed -n '2,30p' "${BASH_SOURCE[0]}"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

if [ ! -f "$corpus" ]; then
  echo "no corpus at $corpus" >&2
  exit 1
fi

# The corpus with comments and blank lines stripped, so line N here is request
# N everywhere else. Written once to a temp file so awk and the loop agree.
requests="$(mktemp)"
trap 'rm -f "$requests"' EXIT
grep -v '^[[:space:]]*#' "$corpus" | grep -v '^[[:space:]]*$' > "$requests"
total_requests="$(wc -l < "$requests" | tr -d ' ')"

header=$'n\trequest\tresult\tstage\tstage_name\tseconds\tkrate_bytes\tlib_lines\tnote'
if [ ! -f "$results" ]; then
  mkdir -p "$(dirname "$results")"
  printf '%s\n' "$header" > "$results"
fi

# Turn a check-app exit code into the stage name it stands for. `create` exits
# 1 on any failure, so we re-run check-app on the leftover work dir to learn
# where it actually stopped -- that is the number worth counting.
stage_name() {
  case "$1" in
    0) echo "-" ;;
    10) echo "layout" ;;
    11) echo "manifest" ;;
    12) echo "build" ;;
    13) echo "imports" ;;
    14) echo "run" ;;
    15) echo "shoot" ;;
    *) echo "other" ;;
  esac
}

print_summary() {
  if [ ! -f "$results" ]; then
    echo "no results yet at $results"
    return
  fi
  awk -F'\t' '
    NR == 1 { next }
    {
      total++
      if ($3 == "pass") { pass++ }
      else if ($3 == "skipped") { skipped++ }
      else { fail++; stage[$5]++ }
      secs += $6
      if ($3 == "pass") { lines += $8; passlines++ }
    }
    END {
      if (total == 0) { print "no results yet"; exit }
      printf "\n  requests run     %d\n", total
      printf "  passed           %d\n", pass
      printf "  failed           %d\n", fail
      if (skipped > 0) printf "  skipped (quota)  %d\n", skipped
      # Pass rate counts only requests that actually reached the AI. A quota
      # rejection means nothing was authored, so including it would understate
      # the system rather than measure it.
      attempted = pass + fail
      if (attempted > 0) printf "  pass rate        %.0f%% (of %d attempted)\n", (pass * 100) / attempted, attempted
      printf "  mean wall time   %.0fs\n", secs / total
      if (passlines > 0) printf "  mean lines (pass) %.0f\n", lines / passlines
      if (fail > 0) {
        printf "\n  failures by stage\n"
        for (s in stage) printf "    %-10s %d\n", s, stage[s]
      }
      print ""
    }
  ' "$results"
}

if [ "$summary_only" = "1" ]; then
  print_summary
  exit 0
fi

if [ ! -x "$krate_bin" ]; then
  echo "no krate binary at $krate_bin -- run: cargo build --release -p krate-cli" >&2
  exit 1
fi

# Which corpus numbers to attempt, in order. --only wins; otherwise it is a
# window starting at --from, and --count 0 means "to the end".
targets=""
if [ -n "$only" ]; then
  targets="$(printf '%s' "$only" | tr ',' ' ')"
else
  last="$total_requests"
  if [ "$count" -gt 0 ]; then
    last=$(( from + count - 1 ))
    [ "$last" -gt "$total_requests" ] && last="$total_requests"
  fi
  targets="$(seq "$from" "$last")"
fi

mkdir -p "$work_root"
echo "krate binary : $krate_bin"
echo "corpus       : $corpus ($total_requests requests)"
echo "results      : $results"
echo "work dirs    : $work_root"
echo

attempted=0
for n in $targets; do
  request="$(awk -v n="$n" 'NR == n' "$requests")"
  if [ -z "$request" ]; then
    echo "[$n] no such request in the corpus, skipping"
    continue
  fi
  # Resumable: a number already in the results file is done, however it went.
  if awk -F'\t' -v n="$n" 'NR > 1 && $1 == n { found = 1 } END { exit !found }' "$results"; then
    echo "[$n] already recorded, skipping"
    continue
  fi

  attempted=$(( attempted + 1 ))
  dir="$work_root/req-$n"
  rm -rf "$dir"
  mkdir -p "$dir"
  out="$dir/app.krate"
  log="$dir/create.log"

  echo "[$n] $request"
  start="$(date +%s)"
  KRATE_AUTHOR_TIMEOUT_SECS="$timeout_secs" \
    "$krate_bin" create "$request" \
      --output "$out" \
      --agent claude \
      --work-dir "$dir/work" \
      > "$log" 2>&1
  create_exit=$?
  end="$(date +%s)"
  seconds=$(( end - start ))

  # The app dir the agent worked in. create names it after the request, so find
  # the one directory holding a Cargo.toml rather than guessing the name.
  app_dir="$(find "$dir/work" -maxdepth 2 -name Cargo.toml -print 2>/dev/null | head -1 | xargs -I{} dirname {})"

  krate_bytes=0
  [ -f "$out" ] && krate_bytes="$(wc -c < "$out" | tr -d ' ')"
  lib_lines=0
  [ -n "$app_dir" ] && [ -f "$app_dir/src/lib.rs" ] && \
    lib_lines="$(wc -l < "$app_dir/src/lib.rs" | tr -d ' ')"

  # A rate-limit rejection is not an authoring failure: the agent process
  # started, the API refused it, and no code was ever written. Counting those
  # as failures is how a run of 47 quota rejections turned a 14/14 pass rate
  # into a reported 23%, which was a lie about the system. Record them as
  # skipped and stop, because once the quota is gone every remaining request
  # fails in two seconds and the whole corpus burns in a minute.
  rate_limited=0
  if [ -n "$app_dir" ] && [ -f "$app_dir/.agent-transcript.txt" ] && \
     grep -q '"status":"rejected"' "$app_dir/.agent-transcript.txt" 2>/dev/null; then
    rate_limited=1
  fi

  if [ "$rate_limited" = "1" ]; then
    result="skipped"
    stage=0
    note="rate limited: the AI account was out of quota, no code was written"
  elif [ "$create_exit" = "0" ]; then
    result="pass"
    stage=0
    note="-"
  else
    result="fail"
    # `create` exits 1 whatever went wrong, so ask the oracle where it stopped.
    # No work dir at all means it never got as far as authoring.
    if [ -n "$app_dir" ]; then
      "$krate_bin" check-app "$app_dir" > "$dir/check.log" 2>&1
      stage=$?
    else
      stage=10
    fi
    note="$(grep -m1 -iE 'error|failed|did not' "$log" | cut -c1-160 | tr '\t\n' '  ')"
    [ -z "$note" ] && note="-"
  fi
  name="$(stage_name "$stage")"

  # Tabs are the field separator, so scrub any from the free text fields.
  clean_request="$(printf '%s' "$request" | tr '\t' ' ')"
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$n" "$clean_request" "$result" "$stage" "$name" \
    "$seconds" "$krate_bytes" "$lib_lines" "$note" >> "$results"

  echo "     -> $result ${seconds}s stage=$name lines=$lib_lines"

  if [ "$rate_limited" = "1" ]; then
    echo
    echo "Stopping: the AI account is out of quota."
    echo "Nothing is lost -- this run is resumable, so re-run it when the quota"
    echo "resets and it will pick up from request $n."
    break
  fi

  # Drop the build artifacts, keep the source. Every app dir compiles its own
  # copy of the SDK into a private target/ of a gigabyte or so, and a corpus run
  # is a hundred of them -- that fills a disk long before the run finishes, and
  # a full disk fails requests for a reason that has nothing to do with
  # authoring. The source, the manifest, the .krate, and the transcript all stay
  # for inspection; only the rebuildable part goes.
  if [ "${KEEP_BUILD_DIRS:-0}" != "1" ] && [ -n "$app_dir" ]; then
    rm -rf "$app_dir/target"
  fi
done

echo
echo "attempted $attempted request(s) this run"
print_summary
