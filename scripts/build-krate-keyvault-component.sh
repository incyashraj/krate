#!/usr/bin/env sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
# One shared toolchain preflight: resolve the Rust rust-toolchain.toml
# declares, verify that is really what `cargo` now runs, and stop with a
# repair line otherwise (IC-683). Every script used to carry its own copy of
# this dance, asked from the wrong directory and verified by nobody.
. "$ROOT/scripts/rust-env.sh"

cd "$ROOT/apps/krate-keyvault"
. "$ROOT/scripts/check-lock-current.sh"
krate_check_lock_current "$PWD"
cargo-component build --release --locked

echo "$ROOT/apps/krate-keyvault/target/wasm32-wasip1/release/krate_keyvault.wasm"
