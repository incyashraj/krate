#!/usr/bin/env sh
# Re-port the programs we have already proven, and check they still work.
#
# Two real third-party programs were ported by hand and the results written
# down. Nothing re-ran them, so a change that broke porting would have been
# found by a user rather than by us -- and the sources only existed in /tmp,
# which a reboot clears.
#
# This clones each one at a pinned commit, ports it, and asserts the result
# builds, packs, runs, and prints what it printed before. A regression anywhere
# in the SDK, the analyzer, the runtime, or the contract fails this script.
#
#   sh scripts/port-regression.sh [workdir]
#
# Needs a network (it clones) and an AI agent for the transform step, so it is
# a nightly job rather than something on every push.
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
WORK="${1:-${TMPDIR:-/tmp}/krate-port-regression}"
KRATE="${KRATE_BIN:-$ROOT/target/release/krate}"
AGENT="${KRATE_PORT_AGENT:-claude}"

if [ ! -x "$KRATE" ]; then
  echo "no krate binary at $KRATE; run: cargo build --release -p krate-cli" >&2
  exit 1
fi

# The transform step drives an AI agent, which a CI runner does not have. Say so
# and stop, rather than failing every night for a reason that is not a
# regression -- a job that is always red is a job everyone learns to ignore.
if ! command -v "$AGENT" >/dev/null 2>&1; then
  echo "no '$AGENT' on PATH, so the transform step cannot run."
  echo "This checks that porting still works end to end and needs an agent."
  echo "Skipping without failing; run it where the agent is installed."
  exit 0
fi

rm -rf "$WORK"
mkdir -p "$WORK"

failures=0
passed=0

# Each case: name, repository, pinned commit, arguments, and a string the
# output must contain. Pinned rather than tracking main, so a change upstream
# cannot turn our regression test red for a reason that is not ours.
run_case() {
  name="$1"
  repo="$2"
  commit="$3"
  run_args="$4"
  expect="$5"
  grants="$6"

  echo ""
  echo "=== $name ==="
  src="$WORK/$name-src"
  if ! git clone -q "$repo" "$src" 2>/dev/null; then
    echo "  SKIP: could not clone $repo (no network?)"
    return 0
  fi
  ( cd "$src" && git checkout -q "$commit" 2>/dev/null ) || {
    echo "  SKIP: pinned commit $commit not found in $repo"
    return 0
  }

  bundle="$WORK/$name.krate"
  if ! "$KRATE" port "$src" \
      --prepare "$WORK/$name-work" \
      --agent "$AGENT" \
      --to "$bundle" > "$WORK/$name.log" 2>&1; then
    echo "  FAIL: the port did not complete"
    tail -15 "$WORK/$name.log" | sed 's/^/    /'
    failures=$((failures + 1))
    return 0
  fi

  # The bundle has to exist and be a real size, not an empty file.
  bytes="$(wc -c < "$bundle" | tr -d ' ')"
  if [ "$bytes" -lt 1000 ]; then
    echo "  FAIL: bundle is only $bytes bytes"
    failures=$((failures + 1))
    return 0
  fi

  # And it has to run and produce what it produced before. A port that builds
  # but computes something different is the failure this is really watching
  # for -- everything else is caught by the build.
  rundir="$WORK/$name-run"
  mkdir -p "$rundir"
  # The granted subtree, and a file inside it. Ports are granted `input/**`, so
  # the fixture lives where the grant points -- and the directory has to exist
  # before the app runs, because `fs.mkdir` does not create parents.
  mkdir -p "$rundir/input"
  printf 'Hello, Krate!' > "$rundir/input/sample.bin"
  # shellcheck disable=SC2086
  if ! out="$( cd "$rundir" && "$KRATE" run $grants "$bundle" -- $run_args 2>&1 )"; then
    echo "  FAIL: the ported app did not run"
    echo "$out" | tail -8 | sed 's/^/    /'
    failures=$((failures + 1))
    return 0
  fi
  case "$out" in
    *"$expect"*)
      echo "  ok: ported, packed, ran ($bytes bytes), output contains '$expect'"
      passed=$((passed + 1))
      ;;
    *)
      echo "  FAIL: output did not contain '$expect'"
      echo "$out" | head -8 | sed 's/^/    /'
      failures=$((failures + 1))
      ;;
  esac
}

# The two shapes proven so far. Add a case every time a new shape is proven;
# never remove one.
# A GUI app: window, text field, button, computed list, saved state.
run_case "savings" \
  "https://github.com/ahtalbi/bank-savings-calculator.git" \
  "376dc0d053ad36eb7215ea26b10ac8b38c371e6e" \
  "quick" \
  "Rent" \
  "--auto-grant"

# A command-line app that writes bytes, not text: the case that forced
# stdio::write into the SDK.
run_case "hexyl" \
  "https://github.com/sharkdp/hexyl.git" \
  "6ecc29b9c8c84d08a7e860f7f69c22b113b480ea" \
  "input/sample.bin" \
  "48 65 6c 6c 6f" \
  "--auto-grant"

# Filesystem-heavy: walks a directory, reads many files, compares them. The
# case that proved fs.list is a separate grant from fs.read.
run_case "ddh" \
  "https://github.com/darakian/ddh.git" \
  "aac9046fbfe302c64e180a8a88de3c262cd1a1a0" \
  "quick" \
  "Total files" \
  "--auto-grant"

# Network: fetches feeds over HTTPS with a grant per host. Nightly rather than
# per-push, because its work is a real request and a runner without network
# would fail for a reason that is not a regression.
run_case "rssfwd" \\
  "https://github.com/morphy2k/rss-forwarder.git" \\
  "aa0412134687629e415a263536cfb6b6c1207cb4" \\
  "quick" \\
  "rssfwd" \\
  "--auto-grant"

echo ""
echo "================================"
echo "ported and verified: $passed"
echo "failed:              $failures"
if [ "$failures" -gt 0 ]; then
  exit 1
fi
# Skipping every case and reporting success would be a green light that
# verified nothing, which is worse than a red one. If nothing could be cloned,
# say so and fail: the job exists to check that porting still works, and it
# did not check.
if [ "$passed" -eq 0 ]; then
  echo "no case could be ported -- nothing was verified" >&2
  exit 1
fi
