#!/usr/bin/env sh
# The self-service round-trip, headless: create a checklist .krate from scratch
# with a standalone binary, open it, add and toggle items, and reopen it with
# the data persisted — then confirm the permission wall.
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

echo "==> first open: add an item, toggle one, save"
mkdir -p checklist
"$KRATE" run checklist.krate --auto-grant -- quick
first="$(cat checklist/items.txt)"
printf '%s\n' "$first"

echo "==> close and reopen: prior items must load"
before_lines="$(wc -l < checklist/items.txt)"
"$KRATE" run checklist.krate --auto-grant -- quick
after_lines="$(wc -l < checklist/items.txt)"
if [ "$after_lines" -le "$before_lines" ]; then
  echo "reopen should have loaded and grown the saved list" >&2
  exit 1
fi
echo "reopened with $before_lines items persisted, now $after_lines"

echo "==> the wall: withholding fs.write must refuse (exit 5)"
rm -rf checklist && mkdir -p checklist
set +e
"$KRATE" run checklist.krate \
  --grant ui.window:create --grant io.stdout --grant io.args \
  --grant "fs.read:./checklist/**" -- quick >/dev/null 2>&1
denied=$?
set -e
if [ "$denied" != "5" ]; then
  echo "expected exit 5 without fs.write, got $denied" >&2
  exit 1
fi
echo "refused without fs.write (exit 5)"
echo ""
echo "round-trip complete: created, opened, edited, persisted across reopen, and enforced its wall."
