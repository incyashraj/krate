#!/usr/bin/env sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$ROOT"

OUTPUT="target/phase2-ci-stability-evidence/ci-stability-evidence.md"
REPO="${LAYER36_CI_STABILITY_REPO:-incyashraj/layer6x6}"
BRANCH="${LAYER36_CI_STABILITY_BRANCH:-main}"
LIMIT="${LAYER36_CI_STABILITY_LIMIT:-20}"
CREATED_FILTER="${LAYER36_CI_STABILITY_CREATED:-}"
REQUIRE_SUCCESS="${LAYER36_CI_STABILITY_REQUIRE_SUCCESS:-0}"
MIN_SUCCESS_STREAK="${LAYER36_CI_STABILITY_MIN_SUCCESS_STREAK:-1}"

usage() {
  cat <<'USAGE'
Usage: scripts/record-phase2-ci-stability-evidence.sh [--repo <owner/name>] [--branch <branch>] [--limit <n>] [--created <date-filter>] [--require-success] [--min-success-streak <n>] [--output <path>]

Options:
  --repo <owner/name>  GitHub repository to inspect (default: incyashraj/layer6x6)
  --branch <branch>   Branch to inspect (default: main)
  --limit <n>         Number of recent runs per workflow (default: 20)
  --created <date-filter>
                       GitHub run creation filter, such as >=2026-05-18
  --require-success    Exit non-zero unless both tracked workflows have enough green completed runs
  --min-success-streak <n>
                       Minimum completed success streak when --require-success is set (default: 1)
  --output <path>     Output markdown report path

Environment:
  LAYER36_CI_STABILITY_REPO
  LAYER36_CI_STABILITY_BRANCH
  LAYER36_CI_STABILITY_LIMIT
  LAYER36_CI_STABILITY_CREATED
  LAYER36_CI_STABILITY_REQUIRE_SUCCESS
  LAYER36_CI_STABILITY_MIN_SUCCESS_STREAK
USAGE
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --repo)
      if [ "$#" -lt 2 ]; then
        echo "missing value for --repo" >&2
        usage
        exit 2
      fi
      REPO="$2"
      shift 2
      ;;
    --branch)
      if [ "$#" -lt 2 ]; then
        echo "missing value for --branch" >&2
        usage
        exit 2
      fi
      BRANCH="$2"
      shift 2
      ;;
    --limit)
      if [ "$#" -lt 2 ]; then
        echo "missing value for --limit" >&2
        usage
        exit 2
      fi
      LIMIT="$2"
      shift 2
      ;;
    --created)
      if [ "$#" -lt 2 ]; then
        echo "missing value for --created" >&2
        usage
        exit 2
      fi
      CREATED_FILTER="$2"
      shift 2
      ;;
    --require-success)
      REQUIRE_SUCCESS="1"
      shift
      ;;
    --min-success-streak)
      if [ "$#" -lt 2 ]; then
        echo "missing value for --min-success-streak" >&2
        usage
        exit 2
      fi
      MIN_SUCCESS_STREAK="$2"
      shift 2
      ;;
    --output)
      if [ "$#" -lt 2 ]; then
        echo "missing value for --output" >&2
        usage
        exit 2
      fi
      OUTPUT="$2"
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      if [ "$OUTPUT" = "target/phase2-ci-stability-evidence/ci-stability-evidence.md" ]; then
        OUTPUT="$1"
        shift
      else
        echo "unknown argument: $1" >&2
        usage
        exit 2
      fi
      ;;
  esac
done

case "$LIMIT" in
  ''|*[!0-9]*)
    echo "CI stability evidence error: --limit must be a positive integer" >&2
    exit 2
    ;;
esac

if [ "$LIMIT" -lt 1 ]; then
  echo "CI stability evidence error: --limit must be at least 1" >&2
  exit 2
fi

case "$MIN_SUCCESS_STREAK" in
  ''|*[!0-9]*)
    echo "CI stability evidence error: --min-success-streak must be a positive integer" >&2
    exit 2
    ;;
esac

if [ "$MIN_SUCCESS_STREAK" -lt 1 ]; then
  echo "CI stability evidence error: --min-success-streak must be at least 1" >&2
  exit 2
fi

if ! command -v gh >/dev/null 2>&1; then
  echo "CI stability evidence error: gh is required" >&2
  exit 127
fi

mkdir -p "$(dirname "$OUTPUT")"
TMP_DIR="target/phase2-ci-stability-evidence/.tmp"
mkdir -p "$TMP_DIR"

CI_RUNS="$TMP_DIR/ci-runs.tsv"
PAGES_RUNS="$TMP_DIR/pages-runs.tsv"

fetch_runs() {
  workflow="$1"
  output="$2"
  if [ -n "$CREATED_FILTER" ]; then
    gh run list \
      --repo "$REPO" \
      --workflow "$workflow" \
      --branch "$BRANCH" \
      --limit "$LIMIT" \
      --created "$CREATED_FILTER" \
      --json databaseId,createdAt,conclusion,status,displayTitle,url \
      --jq '.[] | [.databaseId,.createdAt,.status,(.conclusion // ""),.displayTitle,.url] | @tsv' \
      >"$output"
  else
    gh run list \
      --repo "$REPO" \
      --workflow "$workflow" \
      --branch "$BRANCH" \
      --limit "$LIMIT" \
      --json databaseId,createdAt,conclusion,status,displayTitle,url \
      --jq '.[] | [.databaseId,.createdAt,.status,(.conclusion // ""),.displayTitle,.url] | @tsv' \
      >"$output"
  fi
}

fetch_runs "CI" "$CI_RUNS"
fetch_runs "Deploy docs to GitHub Pages" "$PAGES_RUNS"

success_streak() {
  file="$1"
  count=0
  tab="$(printf '\t')"
  while IFS="$tab" read -r _id _created status conclusion _title _url; do
    if [ "$status" != "completed" ]; then
      continue
    fi
    if [ "$conclusion" = "success" ]; then
      count=$((count + 1))
    else
      break
    fi
  done <"$file"
  printf '%s' "$count"
}

latest_completed() {
  file="$1"
  tab="$(printf '\t')"
  while IFS="$tab" read -r id created status conclusion title url; do
    if [ "$status" = "completed" ]; then
      printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$id" "$created" "$status" "$conclusion" "$title" "$url"
      return 0
    fi
  done <"$file"
  printf 'n/a\tn/a\tn/a\tn/a\tn/a\tn/a\n'
}

write_workflow_rows() {
  file="$1"
  tab="$(printf '\t')"
  while IFS="$tab" read -r id created status conclusion title url; do
    safe_title="$(printf '%s' "$title" | tr '|' '/')"
    printf '| [%s](%s) | `%s` | `%s` | `%s` | %s |\n' \
      "$id" "$url" "$created" "$status" "${conclusion:-n/a}" "$safe_title"
  done <"$file"
}

ci_streak="$(success_streak "$CI_RUNS")"
pages_streak="$(success_streak "$PAGES_RUNS")"
now_utc="$(date -u +%FT%TZ)"
git_commit="$(git rev-parse --short HEAD 2>/dev/null || printf 'unknown')"

ci_latest="$(latest_completed "$CI_RUNS")"
pages_latest="$(latest_completed "$PAGES_RUNS")"

{
  echo "# Phase 2 CI Stability Evidence"
  echo
  echo "This file is generated by \`scripts/record-phase2-ci-stability-evidence.sh\`."
  echo
  echo "It records the recent hosted CI and Pages runs used during Phase 2 exit review."
  echo "It is evidence, not a completion stamp."
  echo
  echo "## Scope"
  echo
  echo "- Repository: \`$REPO\`"
  echo "- Branch: \`$BRANCH\`"
  echo "- Git commit at recording time: \`$git_commit\`"
  echo "- Generated at (UTC): \`$now_utc\`"
  echo "- Runs inspected per workflow: \`$LIMIT\`"
  if [ -n "$CREATED_FILTER" ]; then
    echo "- Created filter: \`$CREATED_FILTER\`"
  fi
  echo "- Require success: \`$REQUIRE_SUCCESS\`"
  if [ "$REQUIRE_SUCCESS" = "1" ]; then
    echo "- Required completed success streak: \`$MIN_SUCCESS_STREAK\`"
  fi
  echo
  echo "## Summary"
  echo
  echo "| Workflow | Latest completed run | Latest conclusion | Completed success streak |"
  echo "|---|---|---|---:|"
  tab="$(printf '\t')"
  IFS="$tab" read -r ci_id _ci_created _ci_status ci_conclusion ci_title ci_url <<EOF_CI
$ci_latest
EOF_CI
  IFS="$tab" read -r pages_id _pages_created _pages_status pages_conclusion pages_title pages_url <<EOF_PAGES
$pages_latest
EOF_PAGES
  echo "| CI | [$ci_id]($ci_url) $ci_title | \`${ci_conclusion:-n/a}\` | $ci_streak |"
  echo "| Deploy docs to GitHub Pages | [$pages_id]($pages_url) $pages_title | \`${pages_conclusion:-n/a}\` | $pages_streak |"
  echo
  echo "## CI Runs"
  echo
  echo "| Run | Created | Status | Conclusion | Title |"
  echo "|---|---|---|---|---|"
  write_workflow_rows "$CI_RUNS"
  echo
  echo "## Pages Runs"
  echo
  echo "| Run | Created | Status | Conclusion | Title |"
  echo "|---|---|---|---|---|"
  write_workflow_rows "$PAGES_RUNS"
  echo
  echo "## Reading This Report"
  echo
  echo "For Phase 2 exit, the important signal is not one green run. It is a stable"
  echo "pattern of green hosted CI and Pages runs after the final UAPI candidate."
  echo "The self-hosted full gate and fuzz soak remain separate evidence tracks."
} >"$OUTPUT"

echo "wrote $OUTPUT"

if [ "$REQUIRE_SUCCESS" = "1" ] && [ "$ci_streak" -lt "$MIN_SUCCESS_STREAK" ]; then
  echo "CI stability evidence error: CI completed success streak $ci_streak is below required $MIN_SUCCESS_STREAK" >&2
  exit 1
fi

if [ "$REQUIRE_SUCCESS" = "1" ] && [ "$pages_streak" -lt "$MIN_SUCCESS_STREAK" ]; then
  echo "CI stability evidence error: Pages completed success streak $pages_streak is below required $MIN_SUCCESS_STREAK" >&2
  exit 1
fi
