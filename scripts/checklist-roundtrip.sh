#!/usr/bin/env sh
# The self-service round-trip, headless: create a checklist .krate from scratch
# with a standalone binary, open it, add and toggle items, and reopen it with
# the data persisted -- then confirm the permission wall.
#
# This is the machine-checkable version of the external-user flow. The human
# version (double-click, click checkboxes, type into the field, close the window
# with the mouse, reopen) is in docs/book/src/create-and-share.md; this proves
# the same create -> open -> edit -> persist -> refuse path without a GUI.
#
# Usage: scripts/checklist-roundtrip.sh [krate-binary]
set -eu

KRATE="${1:-target/release/krate}"
[ -x "$KRATE" ] || KRATE="target/debug/krate"
KRATE="$(cd "$(dirname "$KRATE")" && pwd)/$(basename "$KRATE")"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cd "$work"
# No KRATE_SDK_ROOT: the binary must materialize its embedded SDK.
unset KRATE_SDK_ROOT

echo "==> create (standalone)"
"$KRATE" create "Make a checklist app that saves locally" --output checklist.krate

# The app keeps its items in its own store now, so the journey is asserted from
# what the app reports rather than by reading a file: there is no file to read,
# and that is the improvement -- a checklist needs no access to your folders.
echo "==> first open: add an item, toggle one, save"
before="$("$KRATE" run checklist.krate --auto-grant -- quick | sed -n 's/^items://p')"
echo "saved $before items"

echo "==> close and reopen: prior items must load"
after="$("$KRATE" run checklist.krate --auto-grant -- quick | sed -n 's/^items://p')"
if [ -z "$before" ] || [ -z "$after" ] || [ "$after" -le "$before" ]; then
  echo "reopen should have loaded and grown the saved list (before=$before after=$after)" >&2
  exit 1
fi
echo "reopened with $before items persisted, now $after"

echo "==> the app keeps nothing in the directory it was run from"
stray="$(ls -A | grep -v '^checklist.krate$' || true)"
if [ -n "$stray" ]; then
  echo "the app should not have written into the working directory: $stray" >&2
  exit 1
fi
echo "nothing written beside the app file"

echo "==> the wall: withholding store.kv must refuse (exit 5)"
set +e
"$KRATE" run checklist.krate \
  --grant ui.window:create --grant io.stdout --grant io.args -- quick >/dev/null 2>&1
denied=$?
set -e
if [ "$denied" != "5" ]; then
  echo "expected exit 5 without store.kv, got $denied" >&2
  exit 1
fi
echo "refused without store.kv (exit 5)"
echo ""
echo "round-trip complete: created, opened, edited, persisted across reopen, and enforced its wall."
