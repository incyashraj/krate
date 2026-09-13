#!/usr/bin/env bash
# Build the bundle validator as WebAssembly and place it where the hub
# Worker imports it (IC-833, K-309: one shared validator, server-side).
#
#   scripts/build-hub-validator.sh
#
# Run before `wrangler deploy` in cloud/worker and before the worker tests:
# the module is NOT committed. ring's C sources need a clang that targets
# wasm32; Apple's does not, Homebrew's llvm and Ubuntu's clang do. Set
# TARGET_CC to override the guess.
set -euo pipefail
cd "$(dirname "$0")/.."

if [ -z "${TARGET_CC:-}" ]; then
  if [ -x /opt/homebrew/opt/llvm/bin/clang ]; then
    export TARGET_CC=/opt/homebrew/opt/llvm/bin/clang
    export TARGET_AR=/opt/homebrew/opt/llvm/bin/llvm-ar
  else
    export TARGET_CC=clang
  fi
fi

cargo build --release --target wasm32-unknown-unknown -p krate-validator-wasm
out=cloud/worker/src/validator.wasm
cp target/wasm32-unknown-unknown/release/krate_validator_wasm.wasm "$out"
printf 'wrote %s (%s bytes)\n' "$out" "$(wc -c < "$out" | tr -d ' ')"
