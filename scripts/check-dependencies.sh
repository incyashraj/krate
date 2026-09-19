#!/usr/bin/env sh
set -eu

# Every lock that reaches a person's machine gets audited, not just the one
# at the root.
#
# The root workspace is the runtime, the CLI and the player. `studio/` is a
# SEPARATE cargo workspace with its own lock, and it ships as an installed
# desktop application -- so a vulnerability there lands on a user exactly as
# one in the root would.
#
# It was not audited by anything. cargo-deny ran from the repo root and never
# saw it, and the drift that hid behind that was real: the root workspace was
# on rustls 0.23.45 while studio's lock sat at 0.23.43, which is
# RUSTSEC-2026-0285 -- TLS 1.3 handshake messages accepted across encryption
# level boundaries, on the path Studio uses to talk to the hub. Dependabot
# reported a different, lesser alert on that lock and never mentioned this
# one. An unaudited lock drifts; that is the whole of it (K-728).
#
# Kept as a list rather than a `find`: a lock that ships is a decision, and a
# new one should be added here deliberately. The apps under apps/ are
# WebAssembly guests that run inside the sandbox with no ambient authority,
# so they are a different risk and not in this list.
LOCKS=". studio"

tmp_log="$(mktemp)"
cleanup() {
  rm -f "$tmp_log"
}
trap cleanup EXIT

for dir in $LOCKS; do
  if [ ! -f "$dir/Cargo.lock" ]; then
    echo "no lock at $dir/Cargo.lock -- nothing to audit there" >&2
    continue
  fi
  echo "== advisories: $dir"
  # The root workspace is held to the full advisory bar. Studio is held to
  # VULNERABILITIES only.
  #
  # Not a lowered standard -- a different tree. Studio's dependencies are
  # Tauri's, and Tauri's tree carries six crates marked unmaintained
  # (proc-macro-error and five unic-*) with no upgrade available to us.
  # Failing the lane on those would mean either blocking every release on
  # somebody else's maintenance, or adding six ignores to deny.toml, which
  # is how an ignore list turns into a place where real findings hide.
  #
  # A vulnerability is the thing that reaches a user, and it still fails
  # here: reintroducing rustls 0.23.43 makes this exit 1 and name
  # RUSTSEC-2026-0285. Checked.
  if (cd "$dir" && cargo deny check advisories 2>&1 | grep -qE "^error\[vulnerability\]"); then
    (cd "$dir" && cargo deny check advisories) >"$tmp_log" 2>&1 || true
    cat "$tmp_log"
    echo "a vulnerability was found in $dir -- see the advisory above" >&2
    exit 1
  fi
  if [ "$dir" != "." ]; then
    echo "no vulnerabilities in $dir (unmaintained warnings from the Tauri tree are not failed on here)"
    continue
  fi
  if (cd "$dir" && cargo deny check advisories) >"$tmp_log" 2>&1; then
    cat "$tmp_log"
  else
    status="$?"
    cat "$tmp_log"
    # These are the advisory database being unreadable, not a clean scan.
    # Saying so and carrying on is deliberate: a database that will not load
    # must not read as "nothing matched".
    if grep -Eq "unsupported CVSS version: 4.0|failed to load advisory database|TOML parse error|failed to acquire advisory database lock|exclusive lock on a read-only path" "$tmp_log"; then
      echo "warning: advisory check skipped for $dir due to current cargo-deny/advisory-db compatibility or local advisory-db lock-path limits; license, bans, and source checks still run."
    else
      exit "$status"
    fi
  fi
done

# Licences, bans and sources stay on the root workspace only. Studio's
# dependency tree is Tauri's, governed by Tauri's own choices, and the
# committed licence map (docs/licence-map.md) is generated from the root
# graph -- running the licence gate over a second graph would fail on
# licences that map has never claimed to cover.
cargo deny check licenses bans sources
