#!/usr/bin/env sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$ROOT"

if [ "${KRATE_OFFLINE:-}" = "1" ]; then
  cargo run -p krate-tools --bin check-phase2-exit-evidence --offline
else
  cargo run -p krate-tools --bin check-phase2-exit-evidence
fi
