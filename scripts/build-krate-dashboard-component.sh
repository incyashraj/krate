#!/usr/bin/env sh
set -eu
ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
# One shared toolchain preflight: resolve the Rust rust-toolchain.toml
# declares, verify that is really what `cargo` now runs, and stop with a
# repair line otherwise (IC-683).
. "$ROOT/scripts/rust-env.sh"
cd "$ROOT/apps/krate-dashboard"
. "$ROOT/scripts/check-lock-current.sh"
krate_check_lock_current "$PWD"
cargo-component build --release --locked
echo "$ROOT/apps/krate-dashboard/target/wasm32-wasip1/release/krate_dashboard.wasm"
