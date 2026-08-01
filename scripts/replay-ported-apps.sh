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

passed=0
failed=0

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
    failed=$((failed + 1))
    return 0
  fi

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
  out="$( cd "$work" && "$KRATE" run --auto-grant "$bundle" -- "$arg" 2>&1 )"
  code="$?"
  set -e

  if [ "$code" -ne 0 ]; then
    echo "  $name: FAILED to run (exit $code)"
    echo "$out" | head -4 | sed 's/^/      /'
    failed=$((failed + 1))
    rm -rf "$work"
    return 0
  fi

  case "$out" in
    *"$expect"*)
      echo "  $name: ok"
      passed=$((passed + 1))
      ;;
    *)
      echo "  $name: ran but did not produce '$expect'"
      echo "$out" | head -4 | sed 's/^/      /'
      failed=$((failed + 1))
      ;;
  esac
  rm -rf "$work"
}

echo "Replaying every ported app:"

# A hex viewer: the case that forced a raw byte write into the SDK.
check "hexyl" "input/sample.bin" "48 65 6c 6c 6f"
# A GUI budget splitter: window, widget tree, saved state.
check "savings" "quick" "Rent"
# A duplicate finder: walks a directory and reads many files.
check "ddh" "quick" "Total files"
# A database CLI: SQL, secrets, and random together.
check "envelope" "quick" "quick"
# The largest port: 5,396 lines of regex generator. Its answer is checkable --
# ^a(?:bc?)?$ matches a, ab, abc and nothing else.
check "grex" "quick" "^a(?:bc?)?"
# The first ported app a non-programmer would open on purpose. `rgba-widen:ok`
# is the decoder turning real bytes into pixels, which is the part that was
# impossible this morning -- there was no way to put a picture in a window.
check "eo2" "quick" "rgba-widen:ok"
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
echo "passed: $passed   failed: $failed"
if [ "$failed" -gt 0 ]; then
  exit 1
fi
if [ "$passed" -eq 0 ]; then
  echo "no bundle was checked -- nothing was verified" >&2
  exit 1
fi
