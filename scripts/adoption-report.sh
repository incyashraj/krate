#!/usr/bin/env bash
#
# The numbers that matter while telling people about Krate, on one screen:
# site views, installs, apps opened, publishes -- per day for two weeks --
# plus GitHub's download counts and repo traffic. Read-only; nothing is
# written anywhere. For the permanent record, run record-adoption.sh.
#
#   scripts/adoption-report.sh
#
set -uo pipefail

hub="${KRATE_HUB:-https://hub.krate.tech}"

stats_file="$(mktemp)"
trap 'rm -f "$stats_file"' EXIT
curl -fsS --max-time 30 "$hub/stats" -o "$stats_file" 2>/dev/null || {
  echo "could not reach $hub/stats" >&2
  exit 1
}

STATS_FILE="$stats_file" python3 << 'PY'
import json, os

d = json.load(open(os.environ["STATS_FILE"]))
days = (d.get("live") or {}).get("actions_by_day") or {}

print("hub.krate.tech, last 14 days")
print(f"{'day':<12}{'views':>7}{'installs':>9}{'opens':>7}{'failed':>7}{'makes':>6}{'publishes':>10}")
totals = {}
for day in sorted(days)[-14:]:
    row = days[day]
    for k, v in row.items():
        totals[k] = totals.get(k, 0) + v
    print(f"{day:<12}{row.get('view',0):>7}{row.get('install',0):>9}"
          f"{row.get('open',0):>7}{row.get('open-failed',0):>7}"
          f"{row.get('make',0):>6}{row.get('publish',0):>10}")
print(f"{'TOTAL':<12}{totals.get('view',0):>7}{totals.get('install',0):>9}"
      f"{totals.get('open',0):>7}{totals.get('open-failed',0):>7}"
      f"{totals.get('make',0):>6}{totals.get('publish',0):>10}")
print(f"\ndistinct installs (90d): "
      f"{(d.get('live') or {}).get('distinct_installs') or d.get('distinct_installs_90d') or 0}")

# Failed opens, by why. `refused` is the permission wall doing its job --
# quote a failure rate WITHOUT it (K-100), or the wall reads as breakage.
reasons = d.get("open_failure_reasons_30d") or d.get("open_failure_reasons") or {}
if reasons:
    print("\nfailed opens by reason (30d) -- `refused` is the wall working, not a failure:")
    for reason, count in sorted(reasons.items(), key=lambda kv: -kv[1]):
        print(f"  {reason:<28}{count:>7}")
PY

if command -v gh >/dev/null 2>&1; then
  echo ""
  echo "GitHub downloads (all releases, cumulative):"
  gh api repos/incyashraj/krate/releases --paginate \
    --jq '[.[].assets[]] | group_by(.name | sub("-[0-9].*$";"")) | map({name: .[0].name | sub("-[0-9].*$";""), n: (map(.download_count) | add)}) | sort_by(-.n) | .[] | "  \(.name)\t\(.n)"' \
    2>/dev/null | column -t -s "$(printf '\t')"
  echo ""
  echo "GitHub repo traffic (rolling 14d):"
  gh api repos/incyashraj/krate/traffic/views --jq '"  page views: \(.count) (\(.uniques) unique)"' 2>/dev/null
  gh api repos/incyashraj/krate/traffic/clones --jq '"  clones:     \(.count) (\(.uniques) unique -- CI inflates this)"' 2>/dev/null
else
  echo ""
  echo "(gh not signed in: GitHub download and traffic numbers skipped)"
fi
