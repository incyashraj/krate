#!/usr/bin/env sh
# The demo, in the order it should be recorded.
#
# Written for filming on a real machine: each step prints what it is about to
# show, runs one command, and pauses. Nothing here is special-cased for the
# demo -- every command is one a person who was sent a .krate would type.
#
# Two commands open real windows and are left for the operator to run by hand,
# because a script cannot judge whether the window looked right. They are
# printed at the end.
#
#   sh scripts/demo-walkthrough.sh            # run it
#   sh scripts/demo-walkthrough.sh --no-pause # for a dry run
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
KRATE="${KRATE_BIN:-$ROOT/target/release/krate}"
BUNDLES="$ROOT/evidence/ported"
PAUSE=1
[ "${1:-}" = "--no-pause" ] && PAUSE=0

if [ ! -x "$KRATE" ]; then
  echo "no krate binary at $KRATE; run: cargo build --release -p krate-cli" >&2
  exit 1
fi

step() {
  echo
  echo "──────────────────────────────────────────────────────────"
  echo "$1"
  echo "──────────────────────────────────────────────────────────"
  # `cmd && { ...; }` as the last line of a function makes the function's exit
  # status that test, and under `set -e` a false test ends the script -- which
  # is why --no-pause stopped after the first heading and printed nothing else.
  if [ "$PAUSE" = "1" ]; then
    printf '  (enter to run) '
    read -r _ || true
  fi
  return 0
}

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$WORK/input" "$WORK/images" "$WORK/documents"
printf 'Hello, Krate!' > "$WORK/input/sample.bin"
cd "$WORK"

step "1. A file arrives. What does it ask for before it runs?
   Nothing is installed. The app states its own access, in plain words."
# Answering 'n' shows the request and the refusal without granting anything --
# the honest first frame, and it returns rather than waiting on a keystroke the
# camera would have to sit through.
# `set -e` plus a pipeline is a trap: the refusal exits 5, and `|| true` on the
# pipeline does not stop the shell reacting to it. Turn the check off for the
# one command whose non-zero exit is the point being demonstrated.
set +e
printf 'n\n' | "$KRATE" run --prompt "$BUNDLES/savings.krate"
set -e

step "2. Granted, it runs. Real output, not a splash screen."
"$KRATE" run --auto-grant "$BUNDLES/savings.krate" -- quick

step "3. The same file, on any of the three systems, is this small."
ls -l "$BUNDLES/savings.krate" | awk '{printf "   %s bytes\n", $5}'
echo "   Apple's Reminders, which keeps lists, is 11.2 MB."

step "4. A 2D game. Gravity, collision, sprites, its own frame loop."
"$KRATE" run --auto-grant "$BUNDLES/bounce.krate" -- quick
ls -l "$BUNDLES/bounce.krate" | awk '{printf "   the whole game: %s bytes\n", $5}'

step "5. The sandbox, tested by attacking it.
   The app is granted /etc, then asked for the system password file."
mkdir -p etc && echo "sandbox copy" > etc/passwd
"$KRATE" run --grant "fs.read:input/**" --grant "fs.read:/etc/**" \
  "$BUNDLES/hexyl.krate" -- /etc/passwd
echo "   Those bytes spell 'sandbox copy'. The real file was never reachable."

step "6. Software that already existed. A markdown viewer from GitHub,
   4,863 lines, now one file that runs everywhere."
"$KRATE" run --auto-grant "$BUNDLES/mdview.krate" -- quick

step "7. Everything, re-run on this machine right now."
sh "$ROOT/scripts/replay-ported-apps.sh"

echo
echo "──────────────────────────────────────────────────────────"
echo "Run these two by hand, with the camera on the screen:"
echo
echo "  $KRATE run --native-window --auto-grant $BUNDLES/savings.krate"
echo "  $KRATE run --native-window --auto-grant $BUNDLES/bounce.krate"
echo
echo "The first opens a real window with real controls. The second animates."
echo "Close the window to end each one."
echo "──────────────────────────────────────────────────────────"
