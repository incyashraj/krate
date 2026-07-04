#!/usr/bin/env sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$ROOT"

if [ "${KRATE_OFFLINE:-}" = "1" ]; then
  cargo run -p krate-tools --bin check-uapi --offline -- --format markdown \
    > docs/book/src/phase2/uapi-freeze-evidence.md
else
  cargo run -p krate-tools --bin check-uapi -- --format markdown \
    > docs/book/src/phase2/uapi-freeze-evidence.md
fi
