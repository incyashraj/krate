#!/usr/bin/env sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$ROOT"

if [ "$#" -lt 3 ]; then
  echo "usage: scripts/compare-phase2-language-variant-evidence.sh <linux.md> <macos.md> <windows.md>" >&2
  exit 2
fi

LINUX_REPORT="$1"
MACOS_REPORT="$2"
WINDOWS_REPORT="$3"

cargo run -p layer36-tools --bin compare-phase2-language-variant-evidence -- \
  --linux "$LINUX_REPORT" \
  --macos "$MACOS_REPORT" \
  --windows "$WINDOWS_REPORT"
