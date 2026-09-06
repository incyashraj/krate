# The one place a repository script picks its Rust toolchain (IC-683).
#
# Source this; do not run it:
#
#   ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
#   . "$ROOT/scripts/rust-env.sh"
#
# Before this file, thirty-four scripts each carried their own copy of the
# rustup-vs-Homebrew dance, and every copy had the same two holes. They asked
# `rustup which cargo` from wherever the script happened to be invoked, so the
# answer honoured that directory's toolchain override rather than this
# repository's rust-toolchain.toml. And none of them checked what they got:
# on a machine where Homebrew's cargo shadows rustup, a script could build the
# whole workspace with a compiler the repository never declared, and the first
# sign was a failure that read as broken code (the documented trap that has
# cost real time -- `krate doctor` could see it; the scripts could not).
#
# This resolves the toolchain rust-toolchain.toml declares, from the
# repository root so the override applies, puts it first on PATH, and then
# VERIFIES that the cargo now resolving is the declared version. On any
# failure it prints one repair instruction and stops. It never installs or
# replaces anything: a developer's tools are theirs.

# Where the repository is. The sourcing script usually set ROOT already; fall
# back to this file's own location so `. scripts/rust-env.sh` works from the
# repo root too.
if [ -z "${ROOT:-}" ]; then
  ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
fi

KRATE_RUST_CHANNEL="$(sed -n 's/^channel = "\(.*\)"$/\1/p' "$ROOT/rust-toolchain.toml")"
if [ -z "$KRATE_RUST_CHANNEL" ]; then
  echo "error: could not read the declared toolchain from $ROOT/rust-toolchain.toml" >&2
  exit 1
fi

if ! command -v rustup >/dev/null 2>&1; then
  echo "error: this repository builds with Rust $KRATE_RUST_CHANNEL via rustup, and rustup is not installed." >&2
  echo "repair: install rustup from https://rustup.rs, then run: rustup toolchain install $KRATE_RUST_CHANNEL" >&2
  exit 1
fi

# Asked from the repository root, so rust-toolchain.toml's override is what
# answers -- not the default toolchain of wherever the caller stood.
KRATE_CARGO="$(cd "$ROOT" && rustup which cargo 2>/dev/null || true)"
if [ -z "$KRATE_CARGO" ]; then
  echo "error: rustup has no toolchain for the declared Rust $KRATE_RUST_CHANNEL." >&2
  echo "repair: rustup toolchain install $KRATE_RUST_CHANNEL" >&2
  exit 1
fi

PATH="$(dirname -- "$KRATE_CARGO"):$HOME/.cargo/bin:$PATH"
export PATH

# Trust nothing until the shell agrees. The whole failure mode this exists
# for is a different cargo answering the name, so the check is on the command
# that will actually run, after the PATH change.
KRATE_CARGO_VERSION="$(cargo --version 2>/dev/null | awk '{print $2}')"
case "$KRATE_CARGO_VERSION" in
  "$KRATE_RUST_CHANNEL"*) ;;
  *)
    echo "error: cargo resolves to ${KRATE_CARGO_VERSION:-nothing}, not the declared $KRATE_RUST_CHANNEL -- another cargo is shadowing rustup's." >&2
    echo "repair: rustup toolchain install $KRATE_RUST_CHANNEL   (then re-run this script; nothing is installed or changed automatically)" >&2
    exit 1
    ;;
esac
