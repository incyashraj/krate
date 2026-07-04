#!/usr/bin/env sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$ROOT"

if command -v wasm-tools >/dev/null 2>&1; then
  WIT_TOOL="wasm-tools"
elif [ -x "$HOME/.cargo/bin/wasm-tools" ]; then
  WIT_TOOL="$HOME/.cargo/bin/wasm-tools"
else
  echo "error: wasm-tools not found in PATH or \$HOME/.cargo/bin" >&2
  echo "hint: cargo install wasm-tools --locked" >&2
  exit 1
fi

for package_dir in \
  "wit/krate/phase3" \
  "wit/krate/phase3/deps/ui" \
  "wit/krate/phase3/deps/gfx" \
  "wit/krate/phase3/deps/audio"
do
  "$WIT_TOOL" component wit "$package_dir" >/dev/null
done

if [ "${KRATE_OFFLINE:-}" = "1" ]; then
  cargo run -p krate-tools --bin check-phase3-uapi --offline
else
  cargo run -p krate-tools --bin check-phase3-uapi
fi
