#!/usr/bin/env sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$ROOT"

if [ "$#" -ne 3 ]; then
  echo "usage: scripts/compare-phase2-benchmark-evidence.sh <linux-md> <macos-md> <windows-md>" >&2
  exit 2
fi

cargo run -p layer36-tools --bin compare-phase2-benchmark-evidence -- \
  --linux "$1" \
  --macos "$2" \
  --windows "$3"
