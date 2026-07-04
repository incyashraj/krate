#!/usr/bin/env sh
set -eu

# Ensure locally installed cargo subcommands are reachable.
export PATH="$HOME/.cargo/bin:$PATH"
FUZZ_MAX_TOTAL_TIME="${KRATE_FUZZ_MAX_TOTAL_TIME:-30}"
FUZZ_TARGETS="${KRATE_FUZZ_TARGETS:-manifest_parse logical_path_parse policy_match}"

case "$FUZZ_MAX_TOTAL_TIME" in
  ''|*[!0-9]*)
    echo "KRATE_FUZZ_MAX_TOTAL_TIME must be a positive integer (seconds)." >&2
    exit 1
    ;;
esac

if [ "$FUZZ_MAX_TOTAL_TIME" -le 0 ]; then
  echo "KRATE_FUZZ_MAX_TOTAL_TIME must be greater than zero." >&2
  exit 1
fi

if [ "${KRATE_FUZZ_SMOKE_DRY_RUN:-0}" = "1" ]; then
  for target in $FUZZ_TARGETS; do
    echo "cargo-fuzz run $target -- -max_total_time=$FUZZ_MAX_TOTAL_TIME  # nightly-pinned cargo/rustc"
  done
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

for target in $FUZZ_TARGETS; do
  echo "Running cargo-fuzz target '$target' for ${FUZZ_MAX_TOTAL_TIME}s"
  cargo-fuzz run "$target" -- -max_total_time="$FUZZ_MAX_TOTAL_TIME"
done
