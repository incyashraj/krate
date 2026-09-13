#!/usr/bin/env sh
# Run every app we have ported, and check it still does what it did.
#
# The full port regression needs an AI agent to redo the transform, and a CI
# runner does not have one -- so that job skips, and Gate 3 was green having
# verified nothing. This is the half that can run anywhere: the bundles are
# committed, so the runtime, the capability wall, and the bundle format are all
# exercised on every operating system, every night, with no agent involved.
#
# What it does not cover: whether porting still *produces* these bundles. That
# needs the agent. This catches a runtime or format regression the day it lands,
# which is the failure that would break every existing app at once.
#
#   sh scripts/replay-ported-apps.sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
KRATE="${KRATE_BIN:-$ROOT/target/release/krate}"
BUNDLES="$ROOT/evidence/ported"

if [ ! -x "$KRATE" ]; then
  KRATE="$ROOT/target/debug/krate"
fi
if [ ! -x "$KRATE" ]; then
  echo "no krate binary; run: cargo build --release -p krate-cli" >&2
  exit 1
fi

# Say which binary is being used, and warn when the runtime source is newer
# than it. A stale binary makes this script replay the host you built an hour
# ago against the bundles you built an hour ago, and report every app green
# while the change you are actually testing is in neither -- which is exactly
# how a real interface break got a clean run here once.
# Absolute, because every app runs from a temporary directory: a relative
# KRATE_BIN resolved there, every run exited 127, and the matrix recorded
# ten failures of a runtime that was never invoked.
KRATE="$(cd "$(dirname "$KRATE")" && pwd)/$(basename "$KRATE")"
echo "using: $KRATE"
newest_source="$(find "$ROOT/crates" "$ROOT/wit" -name '*.rs' -o -name '*.wit' 2>/dev/null \
  | while read -r f; do [ "$f" -nt "$KRATE" ] && echo "$f"; done | head -1)"
if [ -n "$newest_source" ]; then
  echo "warning: $newest_source is newer than the binary above." >&2
  echo "         rebuild first, or this replays a host that predates your change." >&2
fi

passed=0
failed=0
# The matrix as a file, not only as a log (IC-755). One row per corpus
# bundle: pass, fail, or skip with its reason, so a bundle the replay never
# tries is kept as a skip rather than vanishing into a green tick. The
# header names the host and the commit; scripts/e3-matrix.py turns this
# into registry results and a record the claim rests on.
case "$(uname -s)" in
  Darwin) host_os=macos ;;
  Linux) host_os=ubuntu ;;
  MINGW*|MSYS*|CYGWIN*|Windows*) host_os=windows ;;
  *) host_os="$(uname -s | tr '[:upper:]' '[:lower:]')" ;;
esac
case "$(uname -m)" in
  arm64|aarch64) host_arch=arm64 ;;
  x86_64|amd64) host_arch=x86_64 ;;
  *) host_arch="$(uname -m)" ;;
esac
TSV="${REPLAY_TSV:-$ROOT/evidence/e3/replay-$host_os.tsv}"
mkdir -p "$(dirname "$TSV")"
{
  printf '# os\t%s\n' "$host_os"
  printf '# arch\t%s\n' "$host_arch"
  printf '# commit\t%s\n' "$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || echo unknown)"
  printf '# tree\t%s\n' "$(git -C "$ROOT" rev-parse 'HEAD^{tree}' 2>/dev/null || echo unknown)"
  printf '# ran_at\t%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  printf '# krate\t%s\n' "$("$KRATE" --version 2>/dev/null | head -1)"
} > "$TSV"
checked=""
row() { printf '%s\t%s\t%s\t%s\t%s\n' "$1" "$2" "$3" "$4" "$5" >> "$TSV"; checked="$checked $1"; }
now_ms() { python3 -c 'import time; print(int(time.time()*1000))' 2>/dev/null || echo 0; }

# Each case: bundle, argument, and a string the output must contain. The
# expected string is the app's real answer, not a status line -- a bundle that
# runs and computes the wrong thing is the regression worth catching.
check() {
  name="$1"
  arg="$2"
  expect="$3"

  bundle="$BUNDLES/$name.krate"
  if [ ! -f "$bundle" ]; then
    echo "  $name: MISSING at $bundle"
    row "$name" fail "bundle missing at $bundle" 0 0
    failed=$((failed + 1))
    return 0
  fi
  started="$(now_ms)"

  work="$(mktemp -d)"
  # The subtree ported apps are granted, and a file inside it. `fs.mkdir` does
  # not create parents, so the directory has to exist before the app runs.
  mkdir -p "$work/input" "$work/output" "$work/scan"
  printf 'Hello, Krate!' > "$work/input/sample.bin"
  printf 'the quick brown fox the lazy dog the fox\n' > "$work/input/sample.txt"
  # A feed table for the network app, so it resolves a feed and a sink rather
  # than reporting an empty config and exiting early.
  cat > "$work/input/feeds.toml" <<'TOML'
[feeds.rust]
url = "https://blog.rust-lang.org/feed.xml"
interval = "1h"

[feeds.rust.sink]
type = "discord"
url = "https://discord.com/api/webhooks/test"
TOML

  set +e
  # --headless on purpose. GUI apps open a real window by default now, and a
  # regression harness that pops eleven windows is both slow and different from
  # what it does on a CI runner with no display. What this checks is that each
  # app still computes the right answer.
  out="$( cd "$work" && "$KRATE" run --headless --auto-grant "$bundle" -- "$arg" 2>&1 )"
  code="$?"
  set -e

  elapsed=$(( $(now_ms) - started ))
  if [ "$code" -ne 0 ]; then
    echo "  $name: FAILED to run (exit $code)"
    echo "$out" | head -4 | sed 's/^/      /'
    row "$name" fail "failed to run (exit $code)" "$elapsed" "$code"
    failed=$((failed + 1))
    rm -rf "$work"
    return 0
  fi

  case "$out" in
    *"$expect"*)
      echo "  $name: ok"
      row "$name" pass "printed $expect" "$elapsed" 0
      passed=$((passed + 1))
      ;;
    *)
      echo "  $name: ran but did not produce '$expect'"
      row "$name" fail "ran but did not produce '$expect'" "$elapsed" 0
      echo "$out" | head -4 | sed 's/^/      /'
      failed=$((failed + 1))
      ;;
  esac
  rm -rf "$work"
}

echo "Replaying every ported app:"

# A hex viewer: the case that forced a raw byte write into the SDK.
check "hexyl" "input/sample.bin" "48 65 6c 6c 6f"
# A GUI budget splitter: window, widget tree, saved state. Asserts the split
# rather than a caption -- 2275 out of 6500 is the app doing the arithmetic,
# which a label appearing on screen never proved.
check "savings" "quick" "slice:2275"
# A duplicate finder: walks a directory and reads many files.
check "ddh" "quick" "Total files"
# A database CLI: SQL, secrets, and random together.
check "envelope" "quick" "quick"
# The largest port: 5,396 lines of regex generator. Its answer is checkable --
# ^a(?:bc?)?$ matches a, ab, abc and nothing else.
check "grex" "quick" "^a(?:bc?)?"
# rssfwd is deliberately absent. Given a real feed file it reaches the
# internet, and a nightly check that depends on someone else's server reports
# their uptime rather than our runtime. Its bundle still ships and still opens.
# The first ported app a non-programmer would open on purpose: a decoder
# turning real bytes into pixels. Its quick line was `rgba-widen:ok` until the
# apps moved to the key:value shape the authoring pack now requires, so this
# asserts the app's own ok line instead -- the same fact, current spelling.
check "eo2" "quick" "eo2:ok"
# The port that exposed the no-op agent: its first "success" was the untouched
# scaffold, packaged as if it were the app. This bundle is the honest retry.
# `blocks:` is its markdown fixture surviving the parse-layout-render path --
# a non-zero block count means real content came out the other end.
check "mdview" "quick" "blocks:"
# The first guest ever to draw through gfx.canvas2d. Not a port -- an in-repo
# sample -- but it belongs here for the same reason the others do: it is the
# only nightly check that a guest's draw calls reach real pixels on all three
# systems.
check "chart" "quick" "drawn:yes"
# The first app that moves on its own: sixty frames of time-based physics
# through the redraw path. `animated:yes` failing means an app can no longer
# drive its own frames, which is the difference between a viewer and a game.
# playable, not animated: the pinned line only prints when the scripted
# paddle actually broke bricks, so this proves input, collision and score --
# the game loop -- not merely that pixels moved.
check "bounce" "quick" "playable:yes"
# The first app that draws in 3D: nine cubes, one mesh, depth-tested and lit.
# `rendered3d:yes` failing means the 3D path is broken on some system, which is
# the one thing no unit test can tell us about all three at once.
check "cubes" "quick" "rendered3d:yes"
# A network app: fetches feeds over HTTPS with per-host grants.
#
# Deliberately not run here. Its work is a real HTTPS request, so a check that
# exercised it would fail whenever a runner has no network or the feed host is
# slow -- and this script runs on every push. A test that goes red for reasons
# unrelated to the change is a test people learn to re-run rather than read.
#
# It is covered by the nightly port regression instead, where a network
# dependency is acceptable, and by the cross-system bundle job which opens it
# on all three operating systems.

echo ""
# Every corpus bundle nobody checked is a SKIP row, with the reason. It is
# not counted as a pass and not dropped: the matrix must say it was not tried.
for bundle in "$BUNDLES"/*.krate; do
  name="$(basename "$bundle" .krate)"
  case " $checked " in
    *" $name "*) ;;
    *) row "$name" skip "no replay check defined" 0 0; echo "  $name: skipped (no replay check defined)" ;;
  esac
done
echo "passed: $passed   failed: $failed   matrix: $TSV"
if [ "$failed" -gt 0 ]; then
  exit 1
fi
if [ "$passed" -eq 0 ]; then
  echo "no bundle was checked -- nothing was verified" >&2
  exit 1
fi
