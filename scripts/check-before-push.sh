#!/usr/bin/env sh
# Everything CI will judge that can be judged here, in one pass.
#
# CI is confirmation, not discovery. Learning one fact per round trip is how
# a release takes hours: v0.5.1 spent three CI rounds learning three things a
# script could have said in a second, and on 2026-09-20 a pages deploy went
# red on a " -- " in one string -- which left krate.tech/app serving the
# version from before that whole week's fixes, silently, because a failed
# deploy just leaves the old site up.
#
# So: run this before pushing. It is seconds, it needs no network, and every
# check in it is one CI actually performs.
#
#   sh scripts/check-before-push.sh
#
# What it deliberately does NOT do: the full test suite, the cross build, or
# anything Windows. Those need CI. This is the part that does not.
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

# rustup's toolchain, not Homebrew's. A Homebrew rustc shadows rustup on this
# machine and fails with "requires rustc 1.94" on a tree that builds fine --
# it cost a wasted validator build today alone.
PATH="$HOME/.cargo/bin:$PATH"
export PATH

failed=0
note() {
  printf "  %-34s %s\n" "$1" "$2"
}
run() {
  name="$1"
  shift
  if "$@" >/tmp/krate-check-$$.log 2>&1; then
    note "$name" "ok"
  else
    note "$name" "FAILED"
    sed 's/^/      /' /tmp/krate-check-$$.log | head -25
    failed=1
  fi
  rm -f /tmp/krate-check-$$.log
}

echo "checks CI will repeat:"

# Formatting is its own CI lane, and clippy does not catch it.
run "rustfmt (workspace)" cargo fmt --all -- --check
run "rustfmt (studio)" sh -c 'cd studio && cargo fmt -- --check'
run "clippy" cargo clippy --workspace --all-targets

# The exact command CI's quick lane runs ("Run workspace tests"), four
# minutes on this machine. Until 2026-09-20 this gate ran no cargo tests at
# all, and every quick-lane failure that day -- five runs, all of them tests
# that take seconds -- was something this line would have stopped at the
# desk. A push that fails CI costs the next ninety-minute full run too, so
# four minutes here is the cheap side of that trade.
run "workspace tests (CI quick lane)" cargo test --workspace --all-features

# The studio crate is OUTSIDE the workspace, so `cargo test --workspace`
# never compiles it and CI has no lane for it either. Nothing anywhere ran
# these tests, and one of them sat red in main long enough that the fix it
# contradicted (K-829, macOS keeps the real HOME) had already shipped -- a
# test nobody runs is not a guard, it is a comment that takes time to
# compile. Seconds here, and the only thing that watches Studio's shell.
run "studio tests" sh -c 'cd studio && cargo test'

# The studio web tests. These read studio/ui directly, run in about a second,
# and are exactly what failed the pages deploy -- including the rule that
# there are no dashes in text a person reads.
for t in docs/landing/studio/*-test.mjs; do
  [ -f "$t" ] || continue
  run "site: $(basename "$t" .mjs)" node "$t"
done

# The hub worker's own tests, which need the wasm flag.
for t in cloud/worker/test/*.test.mjs; do
  [ -f "$t" ] || continue
  run "worker: $(basename "$t" .test.mjs)" node --experimental-wasm-modules "$t"
done

# The build service's own tests. CI has run these for a while and this
# gate never did, so a builder change was pushed on faith -- and the
# builder is where the money is spent. They spawn the real server from a
# repo-relative path, so they must run from the repo root, which is where
# this script already is.
for t in cloud/builder/test/*.test.mjs; do
  [ -f "$t" ] || continue
  run "builder: $(basename "$t" .test.mjs)" node --experimental-wasm-modules "$t"
done

# The fuzz crate has its own lockfile, outside the workspace, and the fuzz
# nightly fetches with --locked. The 0.5.1 bump moved the workspace crates
# and left fuzz/Cargo.lock naming 0.4.0, so every nightly since failed at
# "cannot update the lock file" before fuzzing a byte. `cargo metadata
# --locked` resolves the graph and writes nothing.
run "fuzz lockfile is current" cargo metadata --manifest-path fuzz/Cargo.toml --locked --format-version 1

# The checkers that exist precisely so a bump or a drift is learned here.
for c in scripts/check-version-pins.py scripts/check-cross-parity.py; do
  [ -f "$c" ] || continue
  run "$(basename "$c" .py)" python3 "$c"
done

# The stranger's path, both halves. The make half is offline and stubs the
# agent, so it is seconds; the install half needs the network and is left to
# CI.
if [ -x target/debug/krate ]; then
  run "make path (stubbed agent)" sh scripts/test-cold-make.sh
else
  note "make path (stubbed agent)" "skipped: no target/debug/krate"
fi

echo ""
if [ "$failed" -eq 0 ]; then
  echo "ready to push."
else
  echo "do NOT push: something above fails, and CI will say the same thing"
  echo "in twenty minutes instead of now."
  exit 1
fi
