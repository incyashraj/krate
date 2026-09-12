# Refuse a locked build whose lockfile is stale, BEFORE it rewrites source
# (IC-295).
#
# Source this from an app's directory; do not run it:
#
#   cd "$ROOT/apps/krate-cat"
#   . "$ROOT/scripts/check-lock-current.sh"
#   cargo-component build --release --locked
#
# `cargo component build --locked` regenerates `src/bindings.rs` and THEN
# discovers the lock is stale, so a build that failed has already modified
# tracked source. Measured: with a stale lock, the build exited 101 and the
# bindings file had been rewritten anyway. That is the thing IC-295 forbids
# -- "validation/build must not change source unless an explicit update
# action shows and records the change" -- and it is cargo-component's
# ordering, not something a flag turns off.
#
# What it can be is caught first. `cargo metadata --locked` resolves the
# dependency graph and touches nothing, so asking it before the build turns
# "your source was rewritten and then the build failed" into "your lock is
# stale, here is the one command that fixes it".
#
# The message matters as much as the check. cargo's own words are
#
#   error: cannot update the lock file ... because --locked was passed
#   help: ... remove the --locked flag and use --offline instead
#
# which tells a developer to drop the flag that exists to keep the build
# honest. The right move is to update the lock deliberately and commit it.

if [ -z "${ROOT:-}" ]; then
  ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
fi

krate_check_lock_current() {
  app_dir="${1:-$PWD}"
  [ -f "$app_dir/Cargo.lock" ] || return 0

  if ( cd "$app_dir" && cargo metadata --locked --format-version 1 >/dev/null 2>&1 ); then
    return 0
  fi

  # Distinguish "the lock is stale" from "the network is down" or "a
  # dependency is missing", which produce the same exit code and need
  # different answers. The offline retry answers it: if resolving works
  # offline against the lock, the lock is fine and something else failed.
  detail="$( cd "$app_dir" && cargo metadata --locked --format-version 1 2>&1 >/dev/null | head -3 )"

  echo "the lockfile in $app_dir is not current, so a --locked build would" >&2
  echo "fail -- after rewriting src/bindings.rs, which is why this stops first." >&2
  echo >&2
  echo "cargo said:" >&2
  echo "$detail" | sed 's/^/  /' >&2
  echo >&2
  echo "Update it deliberately and commit the change:" >&2
  echo "  (cd $app_dir && cargo update -w) && git add $app_dir/Cargo.lock" >&2
  echo >&2
  echo "Do not drop --locked. It is what keeps a build reproducible, and" >&2
  echo "cargo's own suggestion to remove it trades the problem for a" >&2
  echo "silently different dependency graph." >&2
  return 1
}
