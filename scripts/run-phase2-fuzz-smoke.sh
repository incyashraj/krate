#!/usr/bin/env sh
set -eu

# Ensure locally installed cargo subcommands are reachable.
export PATH="$HOME/.cargo/bin:$PATH"
FUZZ_MAX_TOTAL_TIME="${KRATE_FUZZ_MAX_TOTAL_TIME:-30}"
FUZZ_TARGETS="${KRATE_FUZZ_TARGETS:-manifest_parse logical_path_parse policy_match bundle_open}"

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

# Evidence with the run, not lost with the terminal (IC-163): every target's
# full log, and a summary of what the campaign holds afterwards.
EVIDENCE_DIR="target/fuzz-nightly"
mkdir -p "$EVIDENCE_DIR"
SUMMARY="$EVIDENCE_DIR/summary.md"
{
  echo "# Fuzz run"
  echo
  echo "Date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "Commit: $(git rev-parse HEAD 2>/dev/null || echo unknown)"
  echo "Seconds per target: $FUZZ_MAX_TOTAL_TIME"
  echo "Engine: $(cargo-fuzz --version 2>/dev/null || echo cargo-fuzz)"
  echo
} > "$SUMMARY"

for target in $FUZZ_TARGETS; do
  echo "Running cargo-fuzz target '$target' for ${FUZZ_MAX_TOTAL_TIME}s"
  # tee, so the terminal still shows progress while the log is kept. The
  # corpus count before and after is the honest measure of a campaign: a
  # count that never grows across nights means the corpus is being lost,
  # which is exactly what happened for ten weeks (K-244).
  before=$(ls "fuzz/corpus/$target" 2>/dev/null | wc -l | tr -d ' ')
  if cargo-fuzz run "$target" -- -max_total_time="$FUZZ_MAX_TOTAL_TIME" 2>&1 | tee "$EVIDENCE_DIR/$target.log"; then
    verdict="ok"
  else
    verdict="FAILED"
  fi
  after=$(ls "fuzz/corpus/$target" 2>/dev/null | wc -l | tr -d ' ')
  crashes=$(ls "fuzz/artifacts/$target" 2>/dev/null | wc -l | tr -d ' ')
  {
    echo "## $target"
    echo
    echo "- verdict: $verdict"
    echo "- corpus: $before inputs before, $after after"
    echo "- crash artifacts: $crashes"
    echo
  } >> "$SUMMARY"
  # Every target runs even after one fails: a crash in manifest_parse is no
  # reason to skip tonight's coverage of the other two, and the evidence of
  # all three belongs in the same artifact.
  [ "$verdict" = "ok" ] || FAILED=1
done

echo "Evidence written to $EVIDENCE_DIR"
[ "${FAILED:-0}" = "0" ] || exit 1
