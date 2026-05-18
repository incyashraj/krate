#!/usr/bin/env sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$ROOT"

OUTPUT="target/phase2-exit-bundle/exit-bundle.md"
STRICT="${LAYER36_EXIT_BUNDLE_STRICT:-0}"
INCLUDE_RUST_SDK="${LAYER36_EXIT_BUNDLE_INCLUDE_RUST_SDK:-0}"
INCLUDE_CI_STABILITY="${LAYER36_EXIT_BUNDLE_INCLUDE_CI_STABILITY:-0}"
INCLUDE_SELF_HOSTED="${LAYER36_EXIT_BUNDLE_INCLUDE_SELF_HOSTED:-0}"

usage() {
  cat <<'USAGE'
Usage: scripts/record-phase2-exit-bundle.sh [--strict] [--include-rust-sdk] [--include-ci-stability] [--include-self-hosted] [--output <path>]

Options:
  --strict                Exit non-zero when any included evidence step fails
  --include-rust-sdk      Also run the Rust SDK package evidence recorder
  --include-ci-stability  Also record hosted CI and Pages stability evidence
  --include-self-hosted   Also record self-hosted full-gate evidence
  --output <path>         Output markdown file path

Environment:
  LAYER36_EXIT_BUNDLE_STRICT                1 to exit non-zero on failed included steps
  LAYER36_EXIT_BUNDLE_INCLUDE_RUST_SDK      1 to include Rust SDK package evidence
  LAYER36_EXIT_BUNDLE_INCLUDE_CI_STABILITY  1 to include hosted CI stability evidence
  LAYER36_EXIT_BUNDLE_INCLUDE_SELF_HOSTED   1 to include self-hosted evidence
USAGE
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --strict)
      STRICT="1"
      shift
      ;;
    --include-rust-sdk)
      INCLUDE_RUST_SDK="1"
      shift
      ;;
    --include-ci-stability)
      INCLUDE_CI_STABILITY="1"
      shift
      ;;
    --include-self-hosted)
      INCLUDE_SELF_HOSTED="1"
      shift
      ;;
    --output)
      if [ "$#" -lt 2 ]; then
        echo "missing value for --output" >&2
        usage
        exit 2
      fi
      OUTPUT="$2"
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      if [ "$OUTPUT" = "target/phase2-exit-bundle/exit-bundle.md" ]; then
        OUTPUT="$1"
        shift
      else
        echo "unknown argument: $1" >&2
        usage
        exit 2
      fi
      ;;
  esac
done

mkdir -p "$(dirname "$OUTPUT")"
TMP_DIR="$(dirname "$OUTPUT")/.tmp-$(basename "$OUTPUT")"
mkdir -p "$TMP_DIR"

UAPI_LOG="$TMP_DIR/check-uapi.log"
FREEZE_LOCK_LOG="$TMP_DIR/check-uapi-freeze-lock.log"
FREEZE_DECISION_LOG="$TMP_DIR/check-phase2-freeze-decision.log"
ADAPTER_LOG="$TMP_DIR/check-adapter-boundary.log"
EXIT_LEDGER_LOG="$TMP_DIR/check-phase2-exit-evidence.log"
CLOSEOUT_DOCS_LOG="$TMP_DIR/check-phase2-closeout-docs.log"
DOCS_LOG="$TMP_DIR/mdbook.log"
DEPENDENCY_LOG="$TMP_DIR/dependency-evidence.log"
DEPENDENCY_REPORT="$TMP_DIR/dependency-evidence.md"
GO_READINESS_LOG="$TMP_DIR/go-readiness-evidence.log"
GO_READINESS_REPORT="$TMP_DIR/go-readiness-evidence.md"
CI_STABILITY_LOG="$TMP_DIR/ci-stability-evidence.log"
CI_STABILITY_REPORT="$TMP_DIR/ci-stability-evidence.md"
SELF_HOSTED_LOG="$TMP_DIR/self-hosted-evidence.log"
SELF_HOSTED_REPORT="$TMP_DIR/self-hosted-evidence.md"
SDK_LOG="$TMP_DIR/rust-sdk-evidence.log"
SDK_REPORT="$TMP_DIR/rust-sdk-evidence.md"

if scripts/check-uapi.sh >"$UAPI_LOG" 2>&1; then
  UAPI_CODE=0
else
  UAPI_CODE=$?
fi

if scripts/check-uapi-freeze-lock.sh >"$FREEZE_LOCK_LOG" 2>&1; then
  FREEZE_LOCK_CODE=0
else
  FREEZE_LOCK_CODE=$?
fi

if scripts/check-phase2-freeze-decision.sh >"$FREEZE_DECISION_LOG" 2>&1; then
  FREEZE_DECISION_CODE=0
else
  FREEZE_DECISION_CODE=$?
fi

if scripts/check-adapter-boundary.sh >"$ADAPTER_LOG" 2>&1; then
  ADAPTER_CODE=0
else
  ADAPTER_CODE=$?
fi

if scripts/check-phase2-exit-evidence.sh >"$EXIT_LEDGER_LOG" 2>&1; then
  EXIT_LEDGER_CODE=0
else
  EXIT_LEDGER_CODE=$?
fi

if scripts/check-phase2-closeout-docs.sh >"$CLOSEOUT_DOCS_LOG" 2>&1; then
  CLOSEOUT_DOCS_CODE=0
else
  CLOSEOUT_DOCS_CODE=$?
fi

if command -v mdbook >/dev/null 2>&1; then
  MDBOOK="mdbook"
elif [ -x "$HOME/.cargo/bin/mdbook" ]; then
  MDBOOK="$HOME/.cargo/bin/mdbook"
else
  MDBOOK=""
fi

if [ -n "$MDBOOK" ]; then
  if "$MDBOOK" build docs/book >"$DOCS_LOG" 2>&1; then
    DOCS_CODE=0
  else
    DOCS_CODE=$?
  fi
else
  DOCS_CODE=127
  printf 'mdbook not found in PATH or $HOME/.cargo/bin\n' >"$DOCS_LOG"
fi

if scripts/record-phase2-dependency-evidence.sh --strict --output "$DEPENDENCY_REPORT" >"$DEPENDENCY_LOG" 2>&1; then
  DEPENDENCY_CODE=0
else
  DEPENDENCY_CODE=$?
fi

if scripts/record-phase2-go-readiness-evidence.sh --output "$GO_READINESS_REPORT" >"$GO_READINESS_LOG" 2>&1; then
  GO_READINESS_CODE=0
else
  GO_READINESS_CODE=$?
fi

if [ "$INCLUDE_CI_STABILITY" = "1" ]; then
  if scripts/record-phase2-ci-stability-evidence.sh --require-success --output "$CI_STABILITY_REPORT" >"$CI_STABILITY_LOG" 2>&1; then
    CI_STABILITY_CODE=0
  else
    CI_STABILITY_CODE=$?
  fi
else
  CI_STABILITY_CODE=0
  printf 'Hosted CI stability evidence skipped. Run with --include-ci-stability to include it.\n' >"$CI_STABILITY_LOG"
fi

if [ "$INCLUDE_SELF_HOSTED" = "1" ]; then
  if scripts/record-phase2-self-hosted-evidence.sh --require-success --output "$SELF_HOSTED_REPORT" >"$SELF_HOSTED_LOG" 2>&1; then
    SELF_HOSTED_CODE=0
  else
    SELF_HOSTED_CODE=$?
  fi
else
  SELF_HOSTED_CODE=0
  printf 'Self-hosted full-gate evidence skipped. Run with --include-self-hosted to include it.\n' >"$SELF_HOSTED_LOG"
fi

if [ "$INCLUDE_RUST_SDK" = "1" ]; then
  if scripts/record-phase2-rust-sdk-evidence.sh --strict --output "$SDK_REPORT" >"$SDK_LOG" 2>&1; then
    SDK_CODE=0
  else
    SDK_CODE=$?
  fi
else
  SDK_CODE=0
  printf 'Rust SDK package evidence skipped. Run with --include-rust-sdk to include it.\n' >"$SDK_LOG"
fi

now_utc="$(date -u +%FT%TZ)"
host_os="$(uname -s 2>/dev/null || printf 'unknown')"
host_arch="$(uname -m 2>/dev/null || printf 'unknown')"
git_commit="$(git rev-parse --short HEAD 2>/dev/null || printf 'unknown')"
git_status="$(git status --short 2>/dev/null || true)"

result_of() {
  code="$1"
  if [ "$code" -eq 0 ]; then
    printf 'passed'
  else
    printf 'failed'
  fi
}

included_of() {
  flag="$1"
  if [ "$flag" = "1" ]; then
    printf 'yes'
  else
    printf 'no'
  fi
}

{
  echo "# Phase 2 Exit Bundle"
  echo
  echo "This file is generated by \`scripts/record-phase2-exit-bundle.sh\`."
  echo
  echo "It is a local review bundle, not a Phase 2 completion stamp. It collects"
  echo "the cheap checks that should be green before a final exit review."
  echo
  echo "## Host"
  echo
  echo "- Git commit: \`$git_commit\`"
  echo "- Host: \`$host_os\` / \`$host_arch\`"
  echo "- Generated at (UTC): \`$now_utc\`"
  echo "- Rust SDK package evidence included: \`$(included_of "$INCLUDE_RUST_SDK")\`"
  echo "- Hosted CI stability evidence included: \`$(included_of "$INCLUDE_CI_STABILITY")\`"
  echo "- Self-hosted full-gate evidence included: \`$(included_of "$INCLUDE_SELF_HOSTED")\`"
  echo
  echo "## Command Results"
  echo
  echo "| Step | Exit code | Result |"
  echo "|---|---:|---|"
  echo "| UAPI contract check (\`scripts/check-uapi.sh\`) | $UAPI_CODE | $(result_of "$UAPI_CODE") |"
  echo "| UAPI freeze lock check (\`scripts/check-uapi-freeze-lock.sh\`) | $FREEZE_LOCK_CODE | $(result_of "$FREEZE_LOCK_CODE") |"
  echo "| UAPI freeze decision check (\`scripts/check-phase2-freeze-decision.sh\`) | $FREEZE_DECISION_CODE | $(result_of "$FREEZE_DECISION_CODE") |"
  echo "| Adapter boundary check (\`scripts/check-adapter-boundary.sh\`) | $ADAPTER_CODE | $(result_of "$ADAPTER_CODE") |"
  echo "| Exit ledger check (\`scripts/check-phase2-exit-evidence.sh\`) | $EXIT_LEDGER_CODE | $(result_of "$EXIT_LEDGER_CODE") |"
  echo "| Closeout docs check (\`scripts/check-phase2-closeout-docs.sh\`) | $CLOSEOUT_DOCS_CODE | $(result_of "$CLOSEOUT_DOCS_CODE") |"
  echo "| Docs build (\`mdbook build docs/book\`) | $DOCS_CODE | $(result_of "$DOCS_CODE") |"
  echo "| Dependency evidence (\`scripts/record-phase2-dependency-evidence.sh --strict\`) | $DEPENDENCY_CODE | $(result_of "$DEPENDENCY_CODE") |"
  echo "| Go readiness evidence (\`scripts/record-phase2-go-readiness-evidence.sh\`) | $GO_READINESS_CODE | $(result_of "$GO_READINESS_CODE") |"
  if [ "$INCLUDE_CI_STABILITY" = "1" ]; then
    echo "| Hosted CI stability evidence (\`scripts/record-phase2-ci-stability-evidence.sh --require-success\`) | $CI_STABILITY_CODE | $(result_of "$CI_STABILITY_CODE") |"
  else
    echo "| Hosted CI stability evidence | 0 | skipped |"
  fi
  if [ "$INCLUDE_SELF_HOSTED" = "1" ]; then
    echo "| Self-hosted full-gate evidence (\`scripts/record-phase2-self-hosted-evidence.sh --require-success\`) | $SELF_HOSTED_CODE | $(result_of "$SELF_HOSTED_CODE") |"
  else
    echo "| Self-hosted full-gate evidence | 0 | skipped |"
  fi
  if [ "$INCLUDE_RUST_SDK" = "1" ]; then
    echo "| Rust SDK evidence (\`scripts/record-phase2-rust-sdk-evidence.sh --strict\`) | $SDK_CODE | $(result_of "$SDK_CODE") |"
  else
    echo "| Rust SDK evidence | 0 | skipped |"
  fi
  echo
  echo "## Gate Snapshot"
  echo
  echo "Current gate states are copied from \`docs/book/src/phase2/exit-evidence.md\`:"
  echo
  echo "| Gate | Criterion | Status |"
  echo "|---|---|---|"
  awk -F '|' '/^\| P2E-/ {
    gate=$2
    criterion=$3
    status=$4
    gsub(/^ +| +$/, "", gate)
    gsub(/^ +| +$/, "", criterion)
    gsub(/^ +| +$/, "", status)
    printf "| %s | %s | %s |\n", gate, criterion, status
  }' docs/book/src/phase2/exit-evidence.md
  echo
  echo "## Working Tree"
  echo
  if [ -n "$git_status" ]; then
    echo '```text'
    printf '%s\n' "$git_status"
    echo '```'
  else
    echo "Clean at generation time."
  fi
  echo
  echo "## UAPI Check Log (tail)"
  echo
  echo '```text'
  tail -n 120 "$UAPI_LOG"
  echo '```'
  echo
  echo "## UAPI Freeze Lock Log (tail)"
  echo
  echo '```text'
  tail -n 120 "$FREEZE_LOCK_LOG"
  echo '```'
  echo
  echo "## UAPI Freeze Decision Log (tail)"
  echo
  echo '```text'
  tail -n 120 "$FREEZE_DECISION_LOG"
  echo '```'
  echo
  echo "## Adapter Boundary Log (tail)"
  echo
  echo '```text'
  tail -n 120 "$ADAPTER_LOG"
  echo '```'
  echo
  echo "## Exit Ledger Log (tail)"
  echo
  echo '```text'
  tail -n 120 "$EXIT_LEDGER_LOG"
  echo '```'
  echo
  echo "## Closeout Docs Log (tail)"
  echo
  echo '```text'
  tail -n 120 "$CLOSEOUT_DOCS_LOG"
  echo '```'
  echo
  echo "## Docs Build Log (tail)"
  echo
  echo '```text'
  tail -n 120 "$DOCS_LOG"
  echo '```'
  echo
  echo "## Dependency Evidence Log (tail)"
  echo
  echo '```text'
  tail -n 120 "$DEPENDENCY_LOG"
  echo '```'
  if [ -f "$DEPENDENCY_REPORT" ]; then
    echo
    echo "## Dependency Evidence Summary"
    echo
    sed -n '1,42p' "$DEPENDENCY_REPORT"
  fi
  echo
  echo "## Go Readiness Evidence Log (tail)"
  echo
  echo '```text'
  tail -n 120 "$GO_READINESS_LOG"
  echo '```'
  if [ -f "$GO_READINESS_REPORT" ]; then
    echo
    echo "## Go Readiness Evidence Summary"
    echo
    sed -n '1,54p' "$GO_READINESS_REPORT"
  fi
  echo
  echo "## Hosted CI Stability Evidence Log (tail)"
  echo
  echo '```text'
  tail -n 120 "$CI_STABILITY_LOG"
  echo '```'
  if [ "$INCLUDE_CI_STABILITY" = "1" ] && [ -f "$CI_STABILITY_REPORT" ]; then
    echo
    echo "## Hosted CI Stability Evidence Summary"
    echo
    sed -n '1,44p' "$CI_STABILITY_REPORT"
  fi
  echo
  echo "## Self-Hosted Full-Gate Evidence Log (tail)"
  echo
  echo '```text'
  tail -n 120 "$SELF_HOSTED_LOG"
  echo '```'
  if [ "$INCLUDE_SELF_HOSTED" = "1" ] && [ -f "$SELF_HOSTED_REPORT" ]; then
    echo
    echo "## Self-Hosted Full-Gate Evidence Summary"
    echo
    sed -n '1,44p' "$SELF_HOSTED_REPORT"
  fi
  echo
  echo "## Rust SDK Evidence Log (tail)"
  echo
  echo '```text'
  tail -n 120 "$SDK_LOG"
  echo '```'
  if [ "$INCLUDE_RUST_SDK" = "1" ] && [ -f "$SDK_REPORT" ]; then
    echo
    echo "## Rust SDK Evidence Summary"
    echo
    sed -n '1,40p' "$SDK_REPORT"
  fi
} >"$OUTPUT"

echo "wrote $OUTPUT"

if [ "$STRICT" = "1" ] && {
  [ "$UAPI_CODE" -ne 0 ] ||
  [ "$FREEZE_LOCK_CODE" -ne 0 ] ||
  [ "$FREEZE_DECISION_CODE" -ne 0 ] ||
  [ "$ADAPTER_CODE" -ne 0 ] ||
  [ "$EXIT_LEDGER_CODE" -ne 0 ] ||
  [ "$CLOSEOUT_DOCS_CODE" -ne 0 ] ||
  [ "$DOCS_CODE" -ne 0 ] ||
  [ "$DEPENDENCY_CODE" -ne 0 ] ||
  [ "$CI_STABILITY_CODE" -ne 0 ] ||
  [ "$SELF_HOSTED_CODE" -ne 0 ] ||
  [ "$SDK_CODE" -ne 0 ];
}; then
  exit 1
fi

exit 0
