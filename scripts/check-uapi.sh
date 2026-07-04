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
  "wit/krate/phase2" \
  "wit/krate/phase2/deps/io" \
  "wit/krate/phase2/deps/fs" \
  "wit/krate/phase2/deps/net" \
  "wit/krate/phase2/deps/time" \
  "wit/krate/phase2/deps/locale"
do
  "$WIT_TOOL" component wit "$package_dir" >/dev/null
done

if [ "${KRATE_OFFLINE:-}" = "1" ]; then
  cargo run -p krate-tools --bin check-uapi --offline
else
  cargo run -p krate-tools --bin check-uapi
fi
