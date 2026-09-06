#!/usr/bin/env sh
set -eu
ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
# One shared toolchain preflight: resolve the Rust rust-toolchain.toml
# declares, verify that is really what `cargo` now runs, and stop with a
# repair line otherwise (IC-683).
. "$ROOT/scripts/rust-env.sh"
cd "$ROOT/apps/krate-convert"
cargo-component build --release --locked
echo "$ROOT/apps/krate-convert/target/wasm32-wasip1/release/krate_convert.wasm"
