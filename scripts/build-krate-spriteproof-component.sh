#!/usr/bin/env sh
set -eu
ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
# Shared toolchain preflight: declared Rust, verified, or one repair line (IC-683).
. "$ROOT/scripts/rust-env.sh"
cd "$ROOT/apps/krate-spriteproof"
cargo-component build --release --locked
echo "$ROOT/apps/krate-spriteproof/target/wasm32-wasip1/release/krate_spriteproof.wasm"
