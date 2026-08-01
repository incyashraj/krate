#!/usr/bin/env sh
# Type-check the platform-specific crates as another operating system sees them.
#
# This exists because a green local run says nothing about the other two
# systems, and finding that out from CI costs fifteen minutes per attempt.
#
# The failure it catches: a test in the macOS adapter called a function inside
# `mod platform`, which is `#[cfg(target_os = "macos")]`. On macOS everything
# compiles. On Windows the test still compiles -- `#[cfg(test)]` is not
# platform-gated -- and the function it calls does not exist. The Windows job
# reported "cannot find function ns_image_from_rgba" long after the change
# looked finished.
#
# Only the pure-Rust crates are checked. A full cross-build fails on C
# dependencies (zstd-sys and friends) that need a Linux toolchain this machine
# does not have, and installing one to catch a cfg mistake is the wrong trade.
# What matters here is whether the *Rust* still type-checks with a different
# target_os, which is exactly where these bugs live.
#
#   sh scripts/check-other-platforms.sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$ROOT"

# A target that is not this machine. Linux is the cheapest to have installed;
# the point is only that `target_os` differs, so a macOS-gated item disappears.
TARGET="x86_64-unknown-linux-gnu"

if ! rustup target list --installed 2>/dev/null | grep -q "^$TARGET$"; then
  echo "target $TARGET is not installed; skipping the cross-platform check" >&2
  echo "install it with: rustup target add $TARGET" >&2
  exit 0
fi

passed=0
failed=0

# krate-layout is left out on purpose: it depends on `alloca`, whose build
# script needs a C compiler for the target. That failure says nothing about
# cfg correctness, and a check that fails for the wrong reason gets ignored.
for crate in krate-adapter-macos krate-adapter-common krate-port; do
  # --all-targets so tests and examples are included: the bug that prompted
  # this lived in a test, and checking the library alone would have missed it.
  if cargo check -q -p "$crate" --all-targets --target "$TARGET" 2>/tmp/krate-xplat-err; then
    echo "  $crate: ok"
    passed=$((passed + 1))
  else
    echo "  $crate: FAILED to type-check as $TARGET"
    grep -E "^error" -A 4 /tmp/krate-xplat-err | head -10 | sed 's/^/      /'
    failed=$((failed + 1))
  fi
done
rm -f /tmp/krate-xplat-err

echo
echo "passed: $passed   failed: $failed"
[ "$failed" -eq 0 ] || exit 1
