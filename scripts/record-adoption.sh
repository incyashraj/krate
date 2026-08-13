#!/usr/bin/env bash
#
# Write today's adoption numbers into the repository, as a permanent record.
#
# Why a file in git rather than a dashboard: a dashboard shows what is true
# now and forgets what was true in March. Outreach needs the shape of the
# line -- "246 installs when we started talking to people, 1,400 six weeks
# later" -- and that only exists if somebody wrote it down every week.
#
# Cloudflare's Analytics Engine keeps 90 days. Anything older than that is
# gone unless it is here.
#
#   scripts/record-adoption.sh            # append today's snapshot
#   scripts/record-adoption.sh --print    # show the numbers, write nothing
#
# Safe to run twice in one day: the same date replaces its own row rather
# than adding a second.

set -uo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out="$repo_root/evidence/adoption/history.tsv"
hub="${KRATE_HUB:-https://hub.krate.tech}"
print_only=0
[ "${1:-}" = "--print" ] && print_only=1

mkdir -p "$(dirname "$out")"

raw="$(curl -fsS --max-time 30 "$hub/stats" 2>/dev/null)" || {
  echo "could not reach $hub/stats" >&2
  exit 1
}

row="$(printf '%s' "$raw" | python3 -c '
import json, sys, datetime

d = json.load(sys.stdin)
live = d.get("live") or {}

# Totals across whatever window each source covers. The KV history is frozen
# at 2026-08-10 and retires over 90 days; live is the Analytics Engine
# dataset, which is the only source that still moves.
def total(by_day, key):
    return sum(day.get(key, 0) for day in (by_day or {}).values())

kv = d.get("actions_by_day") or {}
lv = live.get("actions_by_day") or {}

# Prefer live where it is readable, and say so in the source column, so a
# reader can tell a real zero from a broken query.
if lv:
    src, days = "live", lv
elif "note" in live or "error" in live:
    src, days = "kv-only", kv
else:
    src, days = "kv", kv

print("\t".join(str(x) for x in [
    datetime.date.today().isoformat(),
    src,
    total(days, "view"),
    total(days, "install"),
    total(days, "make"),
    total(days, "open"),
    total(days, "publish"),
    total(days, "open-failed"),
    live.get("distinct_installs") or d.get("distinct_installs_90d") or 0,
]))
')" || { echo "could not read the stats payload" >&2; exit 1; }

header="date	source	views	installs	makes	opens	publishes	open_failed	distinct_installs"

if [ "$print_only" = "1" ]; then
  printf '%s\n%s\n' "$header" "$row" | column -t -s "$(printf '\t')"
  exit 0
fi

[ -f "$out" ] || printf '%s\n' "$header" > "$out"

# Replace today's row if it is already there, so re-running is harmless.
today="$(printf '%s' "$row" | cut -f1)"
tmp="$(mktemp)"
grep -v "^${today}	" "$out" > "$tmp" 2>/dev/null || true
printf '%s\n' "$row" >> "$tmp"
{ head -1 "$tmp"; tail -n +2 "$tmp" | sort; } > "$out"
rm -f "$tmp"

printf '%s\n%s\n' "$header" "$row" | column -t -s "$(printf '\t')"
echo ""
echo "recorded in evidence/adoption/history.tsv -- commit it to keep the record"
