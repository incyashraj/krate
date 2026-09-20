#!/usr/bin/env sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"

if [ -n "${KRATE_HELLO_WASM:-}" ]; then
  HELLO_WASM="$KRATE_HELLO_WASM"
else
  HELLO_WASM="$("$ROOT/scripts/build-hello-component.sh")"
fi

echo "Running Phase 1 tests with $HELLO_WASM"
if [ -n "${KRATE_HELLO_SHA256:-}" ]; then
  echo "Expecting hello component sha256: $KRATE_HELLO_SHA256"
fi

# --nocapture so a test that stops mid-way says where it stopped.
#
# The Windows lane hung for 84 minutes and printed nothing at all: with
# output captured, a test that never returns is indistinguishable from a
# test that never started (K-240). The suite has since been measured on real
# Windows hardware -- the suspected test passes in 15 s there -- so whatever
# stalls on the runner only appears after ~33 minutes of prior test work,
# and the only way to see it is to let the child speak while it runs.
#
# The cost is a noisier log. That is a fair trade against a two-hour
# cancelled run that names no cause.
# One thread on Windows, and one test held out. K-240.
#
# The lane hung with EXACTLY FOUR tests stuck, and a GitHub Windows runner
# has four cores -- so cargo ran four test threads and all four were
# blocked, which is why the suite went silent rather than slow.
#
# Running it single-threaded answered the question that posed. Run
# 34314029118 hung on ONE named test and the log said which:
#   06:01:26  test a_run_report_names_the_exact_artifact_it_ran ... ok
#   06:51:58  test a_sign_off_names_the_build_and_refuses_to_cover_a_real_failure ...
#   06:51:58  ##[error]The operation was canceled.
# It started, printed no result, and hung for 50 minutes with no other
# thread to contend with. So the four were not contending: one test hangs
# and took the other three down with it.
#
# Holding that test out did NOT help, and that is the useful part. With it
# skipped the lane hung again, at the same 90 minutes, on a DIFFERENT test:
# a_path_outside_ascii_is_refused_by_the_binary_people_run, which simply
# occupied the position the skipped one had.
#
# So it is not one cursed test. In that run only 7 of 116 cli tests finished,
# all within 90 milliseconds, and all seven return early without spawning
# anything ("skipping: phase2 smoke fixture not built"). The 8th -- the first
# to actually launch krate.exe and wait for it -- is where it stopped. A
# krate.exe child launched from this harness on a GitHub Windows runner
# sometimes never returns, and .output() waits on it forever. Which test is
# holding the bag is decided by position, not by anything about the test.
#
# The skip is therefore reverted: it cost Windows coverage of `krate accept`
# and bought nothing.
#
# Windows only. The other two lanes are green and fast, and slowing them
# would cost real minutes to learn nothing.
# How much of the suite actually ran (K-269).
#
# A test whose fixture is missing returns early and PASSES, so the count at
# the end of a run reads as coverage when it is not: 121 tests reported ok in
# 234 seconds on a machine where 44 of them did nothing. Each test prints a
# skip line, but cargo captures both streams for a passing test, so those
# lines are invisible without --nocapture -- which is why a bug report about
# this said "ten" before anybody measured it.
#
# The run below already passes --nocapture, so the lines are in the log. This
# just counts them and says so where a person will see it, at the end, next
# to the number they were about to trust.
log="${TMPDIR:-/tmp}/krate-phase1-$$.log"
trap 'rm -f "$log" "$log.code"' EXIT

# The status has to be cargo's, not tee's. `set -e` is on, so a failing run
# would normally end the script here -- `|| status=$?` keeps it alive just
# long enough to print the tally, and the real status is re-raised at the
# bottom. Writing to the log and reading it back afterwards, rather than
# piping, is what keeps the two separable.
status=0
# Arm the watchdog. It existed and had never once fired.
#
# `arm_test_watchdog` (crates/cli/src/main.rs) has been in the binary since
# K-240's first investigation: a krate.exe child that never returns exits
# itself after a ceiling, so the parent's `.output()` returns, one assertion
# fails naming the command, and the suite carries on. It reads
# KRATE_TEST_WATCHDOG_SECS -- and NOTHING SET IT. Not this script, not
# ci.yml. A guard nobody armed is a guard that does not exist, which is why
# the same hang came back on 2026-09-20 and held the v0.5.1 tag for 85
# minutes on a lane whose other two hosts finished in 38 and 48.
#
# 90s, measured rather than guessed.
#
# The first value here was 300, and the run that used it is the evidence for
# this one. The watchdog worked -- it turned an 85-minute silent hang into
# nine recoverable stalls -- but nine stalls at 300s is 47 minutes, and the
# step has 50. It timed out having spent almost its whole budget waiting.
#
# From that run's own timestamps, across 1,098 gaps between test lines on the
# Windows runner:
#
#   median 0s     p95 0s     slowest honest gap 66s
#   stalls: 300, 300, 301, 300, 600, 601, 300  (600 = two in a row)
#
# So nothing legitimate here needs more than about a minute, and every wait
# past that was the hang. 90s clears the slowest real step by half a minute
# and cuts the same nine stalls from 47 minutes to 13.
#
# Deliberately not lower. A runner under load can stretch a genuine step, and
# a watchdog that kills healthy work turns one flaky lane into a lying one.
export KRATE_TEST_WATCHDOG_SECS="${KRATE_TEST_WATCHDOG_SECS:-90}"

# And bound the AGENT PROBE, which the watchdog cannot help with.
#
# `krate ai` probes five providers, 20 seconds each by default. That is right
# on a person's machine. On the Windows runner the agents are on PATH and can
# never answer -- no sign-in, no route to their vendors -- so every probe
# burns its full 20s. Measured in run 35497836673: three stalls of 9 minutes
# each, all immediately after the test that lists them, inside a step with a
# 50 minute budget.
#
# The watchdog does not catch it because 90s is longer than the probe takes:
# nothing is hung, it is just slow on purpose, five times over. 3s is far more
# than a tool that is going to answer needs, and the tests here assert that
# every provider is ACCOUNTED FOR, not that any of them is ready.
export KRATE_PROBE_TIMEOUT_SECS="${KRATE_PROBE_TIMEOUT_SECS:-3}"

# Let the log stream while it runs, instead of only after.
#
# `--nocapture` was added so a hang would say where it stopped, and then the
# whole run was redirected to a file that is only `cat`-ed at the end -- so a
# hung run still printed nothing live. Measured on 2026-09-20: 85 minutes of
# silence after two header lines, on a step whose output was supposedly
# uncaptured. `tee` keeps both properties: a person watching sees progress,
# and the file is still there for the tally below.
#
# The status must stay cargo's, not tee's. This script is `#!/usr/bin/env sh`,
# where PIPESTATUS does not exist, so cargo's exit code is carried out of the
# pipeline by hand: the subshell writes it to a file that is read back after.
# Checked against dash and zsh, not just bash.
code_file="${log}.code"
if [ "${RUNNER_OS:-}" = "Windows" ]; then
  { KRATE_HELLO_WASM="$HELLO_WASM" cargo test --workspace -- \
      --nocapture --test-threads=1 2>&1 || echo "$?" >"$code_file"; } | tee "$log"
else
  { KRATE_HELLO_WASM="$HELLO_WASM" cargo test --workspace -- \
      --nocapture 2>&1 || echo "$?" >"$code_file"; } | tee "$log"
fi
if [ -f "$code_file" ]; then
  status="$(cat "$code_file")"
  rm -f "$code_file"
fi

# The categorised report (IC-706): what cargo's "passed" actually contains --
# tests that reached their assertions, OPTIONAL skips (a language variant
# built by a later step, a platform the test does not cover), and SETUP
# FAILURES (a fixture the lane was supposed to provide and did not). The
# last is the one that matters: it is coverage the lane claims and does not
# have. Six gift-and-wrap tests skipped in every CI run this way, counted
# as passed, until the report was first run over a lane log.
#
# In CI the lane provides every mandatory fixture, so a setup failure there
# is a broken lane and fails it (--strict; test 1421, IC-709). On a
# developer's machine the same report is advice: it says which fixtures
# would widen the run and who builds them, and leaves cargo's status alone.
echo
report_status=0
if [ -n "${CI:-}" ]; then
  python3 "$ROOT/scripts/test-report.py" "$log" --strict || report_status=$?
else
  python3 "$ROOT/scripts/test-report.py" "$log" || report_status=$?
fi
# A report that could not be produced, or that found a hole, must not be
# hidden behind a green cargo run. cargo's own failure still comes first.
if [ "$status" -eq 0 ] && [ "$report_status" -ne 0 ]; then
  echo "the test report did not come out clean (exit $report_status); see above" >&2
  status="$report_status"
fi

exit "$status"
