#!/bin/bash
# Synthetic click round trip for the drawn button, run under `xvfb-run -a`.
# Launches hello-gui with a native winit window, moves the pointer to the
# button (root padding 16 logical, button 160x32 => center 96,32; the window
# maps at the origin under bare Xvfb with no window manager), clicks, and
# expects the component to observe the press and exit 0.
set -u

target/debug/layer36 run \
  --auto-grant \
  --native-window \
  --manifest apps/layer36-hello-gui/manifest.toml \
  apps/layer36-hello-gui/target/wasm32-wasip1/release/layer36_hello_gui.wasm &
APP=$!

# Give the window and the first draw a moment, then click twice for
# robustness (the first movement also seeds CursorMoved before MouseInput).
sleep 5
xdotool mousemove 96 32 click 1 || true
sleep 1
xdotool mousemove 96 32 click 1 || true

wait "$APP"
exit $?
