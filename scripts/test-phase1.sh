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
# One thread on Windows, until K-240 is understood.
#
# The lane hung with EXACTLY FOUR tests stuck, and a GitHub Windows runner
# has four cores -- so cargo ran four test threads and all four were
# blocked, which is why the suite went silent rather than slow. There was no
# fifth thread left to make progress.
#
# Single-threaded turns that into a question with an answer: if it hangs, it
# hangs on ONE named test and the log says which. If it completes, the four
# were contending with each other rather than individually stuck, and the
# contention is the bug.
#
# Windows only. The other two lanes are green and fast, and slowing them
# would cost real minutes to learn nothing.
if [ "${RUNNER_OS:-}" = "Windows" ]; then
  KRATE_HELLO_WASM="$HELLO_WASM" cargo test --workspace -- --nocapture --test-threads=1
else
  KRATE_HELLO_WASM="$HELLO_WASM" cargo test --workspace -- --nocapture
fi
