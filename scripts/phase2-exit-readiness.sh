#!/usr/bin/env sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$ROOT"

if [ "${KRATE_OFFLINE:-}" = "1" ]; then
  cargo run -p krate-tools --bin phase2-exit-readiness --offline -- "$@"
else
  cargo run -p krate-tools --bin phase2-exit-readiness -- "$@"
fi
