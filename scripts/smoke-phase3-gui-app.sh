#!/usr/bin/env sh
# Phase 3 GUI world smoke: build the hello-gui component, verify it imports
# only krate:* interfaces, and run it headlessly through `krate run`.
# Exit code 1 from the app is the expected clean headless outcome (bounded
# event loop, no click). On macOS, run with --native-window manually to see
# the real AppKit window and click the native button (exit 0).
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$ROOT"

sh scripts/build-krate-hello-gui-component.sh

WASM="apps/krate-hello-gui/target/wasm32-wasip1/release/krate_hello_gui.wasm"

if command -v wasm-tools >/dev/null 2>&1; then
  IMPURE="$(wasm-tools component wit "$WASM" | grep -c 'import wasi' || true)"
  if [ "$IMPURE" != "0" ]; then
    echo "hello-gui component is not import-pure: $IMPURE wasi imports" >&2
    exit 1
  fi
  echo "hello-gui component imports only krate:* interfaces"
fi

cargo build -p krate-cli

set +e
target/debug/krate run \
  --auto-grant \
  --manifest apps/krate-hello-gui/manifest.toml \
  "$WASM" \
  -- quick
CODE=$?
set -e

if [ "$CODE" != "1" ]; then
  echo "expected headless clean exit 1 from hello-gui, got $CODE" >&2
  exit 1
fi

echo "Krate Phase 3 GUI app smoke passed (headless clean exit)"
echo "On macOS, try the native window:"
echo "  target/debug/krate run --auto-grant --native-window --manifest apps/krate-hello-gui/manifest.toml $WASM"
