#!/usr/bin/env sh
set -eu

if [ "${LAYER36_FUZZ_SMOKE_DRY_RUN:-0}" = "1" ]; then
  echo "cargo fuzz run manifest_parse -- -max_total_time=30  # with nightly-pinned CARGO/RUSTC"
  echo "cargo fuzz run logical_path_parse -- -max_total_time=30  # with nightly-pinned CARGO/RUSTC"
  echo "cargo fuzz run policy_match -- -max_total_time=30  # with nightly-pinned CARGO/RUSTC"
  exit 0
fi

if ! command -v cargo-fuzz >/dev/null 2>&1; then
  echo "cargo-fuzz is not installed. Install with:"
  echo "  cargo install cargo-fuzz --locked"
  exit 1
fi

if ! command -v rustup >/dev/null 2>&1; then
  echo "rustup is required for nightly fuzz runs."
  exit 1
fi

if ! rustup toolchain list | grep -q "^nightly"; then
  echo "nightly toolchain is required for cargo-fuzz."
  echo "Install with:"
  echo "  rustup toolchain install nightly --profile minimal"
  exit 1
fi

nightly_cargo="$(rustup which --toolchain nightly cargo)"
nightly_rustc="$(rustup which --toolchain nightly rustc)"
nightly_bindir="$(dirname "$nightly_cargo")"

export PATH="$nightly_bindir:$PATH"
export CARGO="$nightly_cargo"
export RUSTC="$nightly_rustc"
export RUSTUP_TOOLCHAIN="nightly"

cargo fuzz run manifest_parse -- -max_total_time=30
cargo fuzz run logical_path_parse -- -max_total_time=30
cargo fuzz run policy_match -- -max_total_time=30
