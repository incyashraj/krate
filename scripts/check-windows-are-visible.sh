#!/usr/bin/env sh
# Do the native adapters create windows that a person can actually see?
#
# Both winit adapters built windows with `.with_visible(false)`, on the
# assumption that every app calls `window.show` afterwards. None of the sample
# apps do -- they create a window and start drawing, which displays on macOS
# and displayed nothing at all on Windows and Linux.
#
# That shipped. Somebody installed Krate on Windows, ran a 3D app, and got a
# frame count printed in their terminal with no window anywhere. Two of the
# three platforms were affected and every test passed, because the smoke tests
# check that a window was *created*, never that it is visible.
#
# A grep is a blunt instrument, and it is the right one here: creating a real
# window needs a display server, so this cannot be a unit test, and the thing
# that went wrong is one literal in one builder chain.
#
#   sh scripts/check-windows-are-visible.sh

set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"

status=0
for adapter in adapter-windows adapter-linux; do
  file="$ROOT/crates/$adapter/src/winit_native.rs"
  [ -f "$file" ] || continue

  if grep -q 'with_visible(false)' "$file"; then
    echo "$adapter creates windows with .with_visible(false)" >&2
    status=1
  fi

  if ! grep -q 'with_visible(true)' "$file"; then
    echo "$adapter never sets window visibility explicitly" >&2
    status=1
  fi
done

if [ "$status" -ne 0 ]; then
  echo "" >&2
  echo "A window an app never shows is a window nobody sees. The sample apps" >&2
  echo "create a window and draw; they do not call window.show, and they are" >&2
  echo "what a new person runs first. If a window must start hidden, make" >&2
  echo "that a per-window option rather than the default for every app." >&2
  exit 1
fi

echo "native windows are visible when created (windows, linux)"
