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
KRATE_HELLO_WASM="$HELLO_WASM" cargo test --workspace -- --nocapture
