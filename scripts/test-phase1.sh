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

KRATE_HELLO_WASM="$HELLO_WASM" cargo test --workspace
