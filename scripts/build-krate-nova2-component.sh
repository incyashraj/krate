#!/usr/bin/env sh
set -eu
ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
# Shared toolchain preflight: declared Rust, verified, or one repair line (IC-683).
. "$ROOT/scripts/rust-env.sh"
cd "$ROOT/apps/krate-nova2"
. "$ROOT/scripts/check-lock-current.sh"
krate_check_lock_current "$PWD"
cargo-component build --release --locked
echo "$ROOT/apps/krate-nova2/target/wasm32-wasip1/release/krate_nova2.wasm"
