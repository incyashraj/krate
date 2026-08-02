#!/usr/bin/env sh
# Re-measure every number in Plan/Claims-We-Can-Make.md.
#
# A claim on a website is a promise, and the fastest way to break one is to
# improve the code. The bundle sizes, widget counts, and interface counts in
# that file all come from things that change; this prints today's values so
# the file can be corrected before someone quotes it in a pitch.
#
# It reports rather than fails: a smaller bundle is good news, not a build
# error. What it prevents is the claim drifting silently.
#
#   sh scripts/check-claims.sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
CLAIMS="$ROOT/Plan/Claims-We-Can-Make.md"

echo "Numbers behind the claims, measured now:"
echo

echo "Bundle sizes"
python3 - "$ROOT" <<'PY'
import pathlib, sys
root = pathlib.Path(sys.argv[1])
bundles = sorted(root.joinpath("evidence/ported").glob("*.krate"))
total = 0
# Discord on this Mac: `du -sk` reports 440,944 KB. The like-for-like peer
# every public page compares against; Reminders was dropped because a
# single-platform first-party app measures nothing. Re-measure with
# scripts/measure-peer-apps.sh, and keep this in step with the pages.
discord = 440_944 * 1024
for bundle in bundles:
    size = bundle.stat().st_size
    total += size
    print(f"  {bundle.stem:<12} {size:>7} bytes   {discord/size:>6.0f}x smaller than Discord")
print()
print(f"  all {len(bundles)} bundles together: {total} bytes ({total/1024:.0f} KB)")
print(f"  Discord (431 MB) is {discord/total:.1f}x that total")
PY

echo
echo "Parity, from the generated tables"
grep -h "fully implemented" "$ROOT/docs/book/src/reference/interface-parity.md" | sed 's/^/  /'
grep -h "widgets work" "$ROOT/docs/book/src/reference/widget-parity.md" | sed 's/^/  /'

echo
echo "Apps that replay nightly on all three systems"
grep -c '^check "' "$ROOT/scripts/replay-ported-apps.sh" | sed 's/^/  /'

echo
echo "If any of these disagree with $CLAIMS, fix the file."
echo "A claim nobody re-checked is the one that ends up in a pitch deck wrong."
