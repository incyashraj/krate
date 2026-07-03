#!/usr/bin/env bash
# One-command Layer36 GUI demo.
#
#   sh scripts/demo-hello-gui.sh
#
# Builds whatever is missing, then runs the hello-gui component. On macOS a
# real native window opens front and center: click the native button within
# 30 seconds and watch the text field flip to "clicked!". On other hosts the
# same portable file runs headless (the Linux/Windows window backends are the
# next milestone), which still proves the artifact itself is portable.
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$ROOT"

WASM="apps/layer36-hello-gui/target/wasm32-wasip1/release/layer36_hello_gui.wasm"

if [ ! -f "$WASM" ]; then
  echo "==> Building the hello-gui component (first run only)..."
  sh scripts/build-layer36-hello-gui-component.sh >/dev/null
fi

if [ ! -x target/debug/layer36 ] && [ ! -x target/debug/layer36.exe ]; then
  echo "==> Building the layer36 CLI (first run only)..."
  cargo build -p layer36-cli
fi

BIN=target/debug/layer36
[ -x target/debug/layer36.exe ] && BIN=target/debug/layer36.exe

if [ "$(uname -s)" = "Darwin" ]; then
  echo ""
  echo "  A native window is about to open, front and center."
  echo "  Click the \"Click me\" button within 30 seconds and watch the"
  echo "  text field change to \"clicked!\"."
  echo ""
  set +e
  "$BIN" run \
    --auto-grant \
    --native-window \
    --manifest apps/layer36-hello-gui/manifest.toml \
    "$WASM"
  CODE=$?
  set -e
else
  echo ""
  echo "  This host has no native window backend yet (macOS only today),"
  echo "  so the same portable file runs headless for a couple of seconds."
  echo ""
  set +e
  "$BIN" run \
    --auto-grant \
    --manifest apps/layer36-hello-gui/manifest.toml \
    "$WASM" \
    -- quick
  CODE=$?
  set -e
fi

echo ""
case "$CODE" in
  0) echo "RESULT: you clicked the native button and the component saw it. ✅" ;;
  1) echo "RESULT: clean run, no click observed (timed out, or headless host)." ;;
  2) echo "RESULT: you closed the window before clicking." ;;
  *) echo "RESULT: unexpected exit code $CODE — something is wrong." ; exit 1 ;;
esac
echo ""
echo "What just ran: one portable WebAssembly component (same bytes on every"
echo "OS) asked Layer36 for a window and widgets; the host lowered them to"
echo "real native controls, and your click traveled back into the component"
echo "as a portable event. Machine-readable variant:"
echo "  $BIN run --json --auto-grant --manifest apps/layer36-hello-gui/manifest.toml $WASM -- quick"
