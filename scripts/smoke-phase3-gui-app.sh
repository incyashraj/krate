#!/usr/bin/env sh
# Phase 3 GUI world smoke: build the hello-gui component, verify it imports
# only layer36:* interfaces, and run it headlessly through `layer36 run`.
# Exit code 1 from the app is the expected clean headless outcome (bounded
# event loop, no click). On macOS, run with --native-window manually to see
# the real AppKit window and click the native button (exit 0).
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$ROOT"

sh scripts/build-layer36-hello-gui-component.sh

WASM="apps/layer36-hello-gui/target/wasm32-wasip1/release/layer36_hello_gui.wasm"

if command -v wasm-tools >/dev/null 2>&1; then
  IMPURE="$(wasm-tools component wit "$WASM" | grep -c 'import wasi' || true)"
  if [ "$IMPURE" != "0" ]; then
    echo "hello-gui component is not import-pure: $IMPURE wasi imports" >&2
    exit 1
  fi
  echo "hello-gui component imports only layer36:* interfaces"
fi

cargo build -p layer36-cli

set +e
target/debug/layer36 run \
  --auto-grant \
  --manifest apps/layer36-hello-gui/manifest.toml \
  "$WASM"
CODE=$?
set -e

if [ "$CODE" != "1" ]; then
  echo "expected headless clean exit 1 from hello-gui, got $CODE" >&2
  exit 1
fi

echo "Layer36 Phase 3 GUI app smoke passed (headless clean exit)"
echo "On macOS, try the native window:"
echo "  target/debug/layer36 run --auto-grant --native-window --manifest apps/layer36-hello-gui/manifest.toml $WASM"
