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
if [ "${RUNNER_OS:-}" = "Windows" ]; then
  KRATE_HELLO_WASM="$HELLO_WASM" cargo test --workspace -- \
    --nocapture --test-threads=1
else
  KRATE_HELLO_WASM="$HELLO_WASM" cargo test --workspace -- --nocapture
fi
