#!/usr/bin/env sh
set -eu

if [ "${LAYER36_FUZZ_SMOKE_DRY_RUN:-0}" = "1" ]; then
  echo "cargo +nightly fuzz run manifest_parse -- -max_total_time=30"
  echo "cargo +nightly fuzz run logical_path_parse -- -max_total_time=30"
  echo "cargo +nightly fuzz run policy_match -- -max_total_time=30"
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

cargo +nightly fuzz run manifest_parse -- -max_total_time=30
cargo +nightly fuzz run logical_path_parse -- -max_total_time=30
cargo +nightly fuzz run policy_match -- -max_total_time=30
