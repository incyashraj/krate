#!/bin/bash
# Synthetic input round trip for the drawn widgets, run under `xvfb-run -a`.
# Launches hello-gui with a native winit window (mapped at the origin under
# bare Xvfb, no window manager; root padding 16 logical). Optionally types
# into the text field (KRATE_EXPECT_TYPED), scrolls the list, selects a row
# (asserted via KRATE_EXPECT_SELECTED), captures a screenshot
# (KRATE_XVFB_SCREENSHOT), clicks the button (center 96,32), and expects the
# component to observe the press and exit 0 — and, when typing or a row
# selection was requested, to report `typed:<text>` / `selected:<label>` on
# stdout.
set -u

OUT="$(mktemp)"
trap 'rm -f "$OUT"' EXIT

target/debug/krate run \
  --auto-grant \
  --native-window \
  --manifest apps/krate-hello-gui/manifest.toml \
  apps/krate-hello-gui/target/wasm32-wasip1/release/krate_hello_gui.wasm \
  >"$OUT" 2>&1 &
APP=$!

# Give the window and the first draw a moment. The first pointer movement
# also seeds CursorMoved before any MouseInput.
sleep 5

# Keyboard round trip. Bare Xvfb has no window manager, so no window ever
# gets X input focus on its own and typed keys would go nowhere: set focus
# explicitly on the app window first, then click the text field (center
# 176,62) so the app-level focus lands on the field, then type. The guest
# renders the text and reports it on exit.
if [ -n "${KRATE_EXPECT_TYPED:-}" ]; then
  WIN_ID="$(xdotool search --name "Krate Hello GUI" 2>/dev/null | head -1 || true)"
  echo "app window id: ${WIN_ID:-not found}"
  if [ -n "$WIN_ID" ]; then
    xdotool windowfocus --sync "$WIN_ID" || true
  fi
  xdotool mousemove 176 62 click 1 || true
  sleep 1
  xdotool type --delay 60 "$KRATE_EXPECT_TYPED" || true
  sleep 1
fi

# Scroll the drawn scroll area (logical y 110..230; wheel over its center)
# so the screenshot shows a mid-list position: X11 wheel-down is button 5.
xdotool mousemove 176 170 || true
xdotool click --repeat 3 5 || true
sleep 1

# Select the middle row of the list view. Row rects come from the layout
# engine: the list sits at y 228..300 and its three 24px rows center at
# y 240 / 264 / 288, so (166,264) is "pick beta". The component maps the
# clicked row id to an index, re-lowers with a new `selected`, and reports
# `selected:<label>` on exit, which is asserted below.
xdotool mousemove 166 264 click 1 || true
sleep 1

# Optional visual evidence: capture the Xvfb root window as a PNG before the
# click ends the app — after typing, so the typed text is in the picture.
# Never fails the proof; screenshots are best-effort.
if [ -n "${KRATE_XVFB_SCREENSHOT:-}" ]; then
  xwd -root -silent | convert xwd:- "$KRATE_XVFB_SCREENSHOT" || true
fi

xdotool mousemove 96 32 click 1 || true
sleep 1
xdotool mousemove 96 32 click 1 || true

wait "$APP"
CODE=$?

cat "$OUT"

if [ -n "${KRATE_EXPECT_TYPED:-}" ]; then
  if ! grep -q "typed:${KRATE_EXPECT_TYPED}" "$OUT"; then
    echo "expected the component to report typed:${KRATE_EXPECT_TYPED}" >&2
    exit 90
  fi
fi

# The row click above must have reached the component. Asserting the
# reported label (not just the screenshot) proves the whole round trip:
# host hit-test, portable pointer event, guest state, re-lowered tree.
if [ -n "${KRATE_EXPECT_SELECTED:-}" ]; then
  if ! grep -q "selected:${KRATE_EXPECT_SELECTED}" "$OUT"; then
    echo "expected the component to report selected:${KRATE_EXPECT_SELECTED}" >&2
    exit 91
  fi
fi

exit $CODE
