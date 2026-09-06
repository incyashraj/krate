#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "${SCRIPT_DIR}/.." && pwd)"
# Shared toolchain preflight: the Rust rust-toolchain.toml declares, put
# first on PATH and verified, or one repair line and stop (IC-683).
ROOT="${REPO_ROOT}"
. "${ROOT}/scripts/rust-env.sh"

for fixture in hello-world print-loop; do
  (
    cd "${REPO_ROOT}/test/integration/${fixture}"
    cargo-component build --release --locked
  )
done

printf '%s\n' "${REPO_ROOT}/test/integration/hello-world/target/wasm32-wasip1/release/hello_world.wasm"
printf '%s\n' "${REPO_ROOT}/test/integration/print-loop/target/wasm32-wasip1/release/print_loop.wasm"
