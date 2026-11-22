#!/usr/bin/env bash
#
# Put krate.tech live from this machine in under a minute (IC-997).
#
#   scripts/ship-site.sh           build exactly as the deploy does, every check, upload
#   scripts/ship-site.sh --quick   the same without the checks; CI runs them on the commit
#
# What it ships is a commit, never loose files: the site tree must be
# committed, and that commit must fast-forward main. It builds that commit
# (scripts/build-site-local.py, the deploy's own steps), uploads only the
# files that changed to Cloudflare (cloud/site), puts the commit on main so
# CI's build of it ends with the same bytes, and reads the live site back.
#
# Running it is the decision to publish: it pushes main.

set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$(pwd)

QUICK=0
for arg in "$@"; do
  case "$arg" in
    --quick) QUICK=1 ;;
    -h|--help) sed -n '3,15p' "$0"; exit 0 ;;
    *) echo "ship-site: unknown option $arg" >&2; exit 2 ;;
  esac
done

say() { printf '\033[1m%s\033[0m\n' "$*"; }
started=$(date +%s)

# 1. A commit, not loose files. These are what the site build reads.
SITE_PATHS=(docs studio/ui scripts .github/workflows/pages.yml cloud/site CHANNEL README.md CHANGELOG.md)
if ! git diff --quiet HEAD -- "${SITE_PATHS[@]}" || [ -n "$(git ls-files --others --exclude-standard -- docs studio/ui cloud/site)" ]; then
  echo "ship-site: the site has changes that are not committed. Commit them, then ship:" >&2
  git status --short -- "${SITE_PATHS[@]}" | head -20 >&2
  exit 1
fi
git fetch -q origin main
if ! git merge-base --is-ancestor origin/main HEAD; then
  echo "ship-site: main has commits this branch does not. Merge origin/main first, then ship." >&2
  exit 1
fi
COMMIT=$(git rev-parse --short HEAD)

# 2. The CLI the store shelf is packed with: this build's, else the installed one.
if [ -z "${KRATE_BIN:-}" ]; then
  for c in "$ROOT/target/release/krate" "$HOME/.krate/bin/krate" "$(command -v krate || true)"; do
    if [ -n "$c" ] && [ -x "$c" ]; then KRATE_BIN="$c"; break; fi
  done
fi
if [ -z "${KRATE_BIN:-}" ]; then
  echo "ship-site: no krate CLI to pack the store shelf with. Install Krate, or set KRATE_BIN." >&2
  exit 1
fi
export KRATE_BIN

# 3. Build it exactly as the deploy does.
say "Building $COMMIT$([ $QUICK = 1 ] && echo ' (quick: CI runs the checks)')"
OUT="$ROOT/cloud/site/.site"
log=$(mktemp)
if ! KRATE_SITE_QUICK=$QUICK python3 scripts/build-site-local.py --out "$OUT" --work "${TMPDIR:-/tmp}/krate-ship-build" >"$log" 2>&1; then
  tail -40 "$log" >&2
  echo "ship-site: the build failed, so nothing was shipped. The lines above say where." >&2
  exit 1
fi
# As the deploy publishes it: no dot-file but .well-known.
find "$OUT" -mindepth 1 -name '.*' ! -path "$OUT/.well-known" -prune -exec rm -rf {} +
test -f "$OUT/.well-known/security.txt" || { echo "ship-site: the build has no .well-known/security.txt" >&2; exit 1; }
cp cloud/site/_headers "$OUT/_headers"
built=$(date +%s)

# 4. Upload what changed.
say "Uploading"
if ! (cd cloud/site && npx --prefix ../worker wrangler deploy >"$log" 2>&1); then
  tail -20 "$log" >&2
  echo "ship-site: the upload failed; the live site is unchanged." >&2
  exit 1
fi
grep -E "Uploaded [0-9]+ files" "$log" || true

# 5. Main carries the same commit, so CI's build of it lands on the same bytes.
#    KRATE_SHIP_NO_PUSH=1 leaves main alone: for trying the script against a
#    preview address (KRATE_SITE_URL) before krate.tech points here.
if [ "${KRATE_SHIP_NO_PUSH:-}" != "1" ] && [ "$(git rev-parse origin/main)" != "$(git rev-parse HEAD)" ]; then
  say "Putting $COMMIT on main"
  git push -q origin HEAD:main
fi

# 6. Read the live site back: the home page must be the one just built.
LIVE="${KRATE_SITE_URL:-https://krate.tech}"
want=$(shasum -a 256 "$OUT/index.html" | cut -d' ' -f1)
got=$(curl -fsS "$LIVE/?shipped=$COMMIT" | shasum -a 256 | cut -d' ' -f1 || true)
if [ "$want" = "$got" ]; then
  say "Live on $LIVE in $(( $(date +%s) - started ))s (build $(( built - started ))s)."
else
  echo "ship-site: uploaded, but $LIVE does not serve this build yet." >&2
  echo "  If krate.tech still points at GitHub Pages, it updates when CI's deploy of $COMMIT ends." >&2
  exit 3
fi
rm -f "$log"
