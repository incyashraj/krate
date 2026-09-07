#!/bin/bash
# Synthetic input round trip for the drawn widgets, run under `xvfb-run -a`.
# Launches hello-gui with a native winit window (mapped at the origin under
# bare Xvfb, no window manager; root padding 16 logical). Optionally types
# into the text field (KRATE_EXPECT_TYPED), scrolls the list, selects a row
# (asserted via KRATE_EXPECT_SELECTED), captures a screenshot
# (KRATE_XVFB_SCREENSHOT), clicks the button (center 96,32), and expects the
# component to observe the press and exit 0 -- and, when typing or a row
# selection was requested, to report `typed:<text>` / `selected:<label>` on
# stdout.
set -u

OUT="$(mktemp)"
SERVE_DIR=""
SERVER_PID=""
WM_PID=""
cleanup() {
  rm -f "$OUT"
  [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null
  [ -n "$WM_PID" ] && kill "$WM_PID" 2>/dev/null
  [ -n "$SERVE_DIR" ] && rm -rf "$SERVE_DIR"
  return 0
}
trap cleanup EXIT

# A window manager, before anything opens a window (K-238).
#
# Bare Xvfb has none, and without one no window ever takes X input focus --
# so xdotool's synthetic clicks and keys go nowhere. The app is found, clicked
# at, and hears none of it, which left this script waiting for an app that
# would never close. The workaround was an explicit `windowfocus --sync`,
# which on a WM-less server waits for a confirmation that never comes.
#
# openbox is the smallest thing that does the job. If it is not installed the
# script carries on exactly as before rather than failing: the assertions
# below still say whether the clicks landed.
if command -v openbox >/dev/null 2>&1; then
  openbox >/dev/null 2>&1 &
  WM_PID=$!
  # Give it a moment to own the root window before any app maps one.
  sleep 1
fi

# KRATE_BUNDLE_URL_PROOF packs hello-gui into a .krate, serves it over local
# HTTP, and runs it by URL instead of from a path. Same app, same assertions:
# the point is that delivering an app as one downloadable file changes nothing
# about how it behaves or what it is allowed to do.
if [ -n "${KRATE_BUNDLE_URL_PROOF:-}" ]; then
  SERVE_DIR="$(mktemp -d)"
  cp apps/krate-hello-gui/target/wasm32-wasip1/release/krate_hello_gui.wasm \
    "$SERVE_DIR/code.wasm"
  sed 's|^entry.*|entry = "code.wasm"|' apps/krate-hello-gui/manifest.toml \
    >"$SERVE_DIR/manifest.toml"

  target/debug/krate pack "$SERVE_DIR/code.wasm" \
    --manifest "$SERVE_DIR/manifest.toml" \
    -o "$SERVE_DIR/hello.krate" || exit 92

  ( cd "$SERVE_DIR" && python3 -m http.server 8899 >/dev/null 2>&1 ) &
  SERVER_PID=$!
  # Wait for the server rather than sleeping a fixed amount.
  for _ in $(seq 1 40); do
    if curl -fsS -o /dev/null "http://127.0.0.1:8899/hello.krate" 2>/dev/null; then
      break
    fi
    sleep 0.25
  done

  echo "serving bundle: $(stat -c%s "$SERVE_DIR/hello.krate") bytes"
  target/debug/krate run \
    --auto-grant \
    --native-window \
    --insecure-http \
    "http://127.0.0.1:8899/hello.krate" \
    >"$OUT" 2>&1 &
  APP=$!
else
  target/debug/krate run \
    --auto-grant \
    --native-window \
    --manifest apps/krate-hello-gui/manifest.toml \
    apps/krate-hello-gui/target/wasm32-wasip1/release/krate_hello_gui.wasm \
    >"$OUT" 2>&1 &
  APP=$!
fi

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
    # `--sync` waits for the focus change to be confirmed, and on bare Xvfb
    # with no window manager that confirmation may never come -- it blocks
    # forever rather than failing, which `|| true` cannot catch. Bounded.
    timeout 15 xdotool windowfocus --sync "$WIN_ID" || true
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
# click ends the app -- after typing, so the typed text is in the picture.
# Never fails the proof; screenshots are best-effort.
if [ -n "${KRATE_XVFB_SCREENSHOT:-}" ]; then
  xwd -root -silent | convert xwd:- "$KRATE_XVFB_SCREENSHOT" || true
fi

xdotool mousemove 96 32 click 1 || true
sleep 1
xdotool mousemove 96 32 click 1 || true

# The app closes when the two clicks above land on its close button. If they
# do not -- no window manager, a missed hit, a changed layout -- the app keeps
# its window open by design (K-092: apps stop closing their own windows) and
# this wait never returns. That is exactly what hung the Linux lane for two
# hours before it was bounded: the window run itself passed, and the script
# sat here (K-238).
#
# 120s is far past the second or two the click path needs. On expiry the app
# is killed and the run is reported as the failure it is, with its output --
# a hang that says nothing is the worst possible outcome for a proof.
if ! timeout 120 tail --pid="$APP" -f /dev/null 2>/dev/null; then
  echo "the app did not exit after the close clicks -- the click path did not reach it" >&2
  kill "$APP" 2>/dev/null || true
  wait "$APP" 2>/dev/null || true
  cat "$OUT"
  exit 91
fi
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
