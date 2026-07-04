#!/usr/bin/env bash
set -euo pipefail

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$ROOT"

if [ "$(uname -s)" != "Darwin" ]; then
  echo "Krate Phase 3 AppKit runtime smoke skipped: host is not macOS"
  exit 0
fi

cargo run -p krate-runtime --example phase3_appkit_runtime_smoke
