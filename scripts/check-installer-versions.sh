#!/usr/bin/env sh
# Do the installers quote a version that still exists?
#
# Both installers print an example command when they cannot reach GitHub's API,
# which happens on any shared address that has used its sixty requests. That
# example pins a version, and a pinned version rots: for two releases the
# suggestion was v0.1.0-rc4 while the newest tag was rc5, so the one instruction
# offered to somebody already having a bad time installed something old.
#
# Nothing else would catch it. The installers are not compiled and not tested by
# the workspace, and the line only runs on a rate-limited network.
#
#   sh scripts/check-installer-versions.sh

set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"

newest="$(git -C "$ROOT" tag --sort=-creatordate | grep '^v' | head -1)"
if [ -z "$newest" ]; then
  echo "no v-tags in this repository yet; nothing to check" >&2
  exit 0
fi

status=0
for file in scripts/install.sh scripts/install.ps1; do
  path="$ROOT/$file"
  [ -f "$path" ] || continue
  # Only lines a person is told to type. Comments explaining the archive naming
  # convention mention old tags on purpose and are not instructions, so a check
  # that flagged them would be noise -- and a noisy check gets ignored, which
  # is how the stale one survived two releases in the first place.
  for quoted in $(grep -v '^[[:space:]]*#' "$path" \
    | grep -oE 'v0\.[0-9]+\.[0-9]+(-rc[0-9]+)?' | sort -u); do
    if [ "$quoted" != "$newest" ]; then
      echo "$file suggests $quoted, but the newest release is $newest" >&2
      status=1
    fi
  done
done

if [ "$status" -ne 0 ]; then
  echo "" >&2
  echo "Update the pinned example, or drop the version and point at" >&2
  echo "https://github.com/incyashraj/krate/releases instead." >&2
  exit 1
fi

echo "installer version examples match the newest tag ($newest)"
