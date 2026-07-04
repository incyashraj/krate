#!/usr/bin/env sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$ROOT"

OUTPUT="target/phase2-hosted-full-ci-evidence/hosted-full-ci-evidence.md"
REPO="${KRATE_HOSTED_FULL_CI_REPO:-incyashraj/krate}"
BRANCH="${KRATE_HOSTED_FULL_CI_BRANCH:-main}"
LIMIT="${KRATE_HOSTED_FULL_CI_LIMIT:-20}"
CREATED_FILTER="${KRATE_HOSTED_FULL_CI_CREATED:-}"
REQUIRE_SUCCESS="${KRATE_HOSTED_FULL_CI_REQUIRE_SUCCESS:-0}"

usage() {
  cat <<'USAGE'
Usage: scripts/record-phase2-hosted-full-ci-evidence.sh [--repo <owner/name>] [--branch <branch>] [--limit <n>] [--created <date-filter>] [--require-success] [--output <path>]

Options:
  --repo <owner/name>  GitHub repository to inspect (default: incyashraj/krate)
  --branch <branch>   Branch to inspect (default: main)
  --limit <n>         Number of recent CI runs to inspect (default: 20)
  --created <filter>  GitHub run creation filter, such as >=2026-05-18
  --require-success   Exit non-zero unless a completed full CI run has all required full jobs green
  --output <path>     Output markdown report path

Environment:
  KRATE_HOSTED_FULL_CI_REPO
  KRATE_HOSTED_FULL_CI_BRANCH
  KRATE_HOSTED_FULL_CI_LIMIT
  KRATE_HOSTED_FULL_CI_CREATED
  KRATE_HOSTED_FULL_CI_REQUIRE_SUCCESS
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
      if [ "$OUTPUT" = "target/phase2-hosted-full-ci-evidence/hosted-full-ci-evidence.md" ]; then
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
    echo "hosted full CI evidence error: --limit must be a positive integer" >&2
    exit 2
    ;;
esac

if [ "$LIMIT" -lt 1 ]; then
  echo "hosted full CI evidence error: --limit must be at least 1" >&2
  exit 2
fi

if ! command -v gh >/dev/null 2>&1; then
  echo "hosted full CI evidence error: gh is required" >&2
  exit 127
fi

mkdir -p "$(dirname "$OUTPUT")"
TMP_DIR="target/phase2-hosted-full-ci-evidence/.tmp"
mkdir -p "$TMP_DIR"

RUNS_TSV="$TMP_DIR/ci-runs.tsv"
JOBS_TSV="$TMP_DIR/jobs.tsv"

if [ -n "$CREATED_FILTER" ]; then
  gh run list \
    --repo "$REPO" \
    --workflow "CI" \
    --branch "$BRANCH" \
    --limit "$LIMIT" \
    --created "$CREATED_FILTER" \
    --json databaseId,createdAt,status,conclusion,headSha,displayTitle,url \
    --jq '.[] | [.databaseId,.createdAt,.status,(.conclusion // ""),.headSha,.displayTitle,.url] | @tsv' \
    >"$RUNS_TSV"
else
  gh run list \
    --repo "$REPO" \
    --workflow "CI" \
    --branch "$BRANCH" \
    --limit "$LIMIT" \
    --json databaseId,createdAt,status,conclusion,headSha,displayTitle,url \
    --jq '.[] | [.databaseId,.createdAt,.status,(.conclusion // ""),.headSha,.displayTitle,.url] | @tsv' \
    >"$RUNS_TSV"
fi

REQUIRED_JOBS='Phase 2 bindings
Build shared component fixtures
Full test (ubuntu-latest)
Full test (macos-latest)
Full test (windows-latest)
Language variant evidence compare
UCap enforcement evidence compare
Adapter evidence compare
Sample evidence compare
Benchmarks (Phase 2 baseline, warning only)
Dependency audit (cargo-deny)'

job_conclusion() {
  job_name="$1"
  awk -F '\t' -v wanted="$job_name" '$1 == wanted { print $3; found = 1; exit } END { if (!found) print "missing" }' "$JOBS_TSV"
}

run_has_required_full_jobs() {
  while IFS= read -r job_name; do
    [ -n "$job_name" ] || continue
    job_result="$(job_conclusion "$job_name")"
    if [ "$job_result" = "missing" ] || [ "$job_result" = "skipped" ]; then
      return 1
    fi
  done <<EOF_JOBS
$REQUIRED_JOBS
EOF_JOBS
  return 0
}

run_full_jobs_green() {
  while IFS= read -r job_name; do
    [ -n "$job_name" ] || continue
    job_result="$(job_conclusion "$job_name")"
    if [ "$job_result" != "success" ]; then
      return 1
    fi
  done <<EOF_JOBS
$REQUIRED_JOBS
EOF_JOBS
  return 0
}

fetch_jobs() {
  run_id="$1"
  gh run view "$run_id" \
    --repo "$REPO" \
    --json jobs \
    --jq '.jobs[] | [.name,.status,(.conclusion // ""),.url] | @tsv' \
    >"$JOBS_TSV"
}

selected_id="n/a"
selected_created="n/a"
selected_status="n/a"
selected_conclusion="n/a"
selected_sha="n/a"
selected_title="n/a"
selected_url="n/a"
selected_full="no"
selected_green="no"

tab="$(printf '\t')"
while IFS="$tab" read -r run_id created status conclusion sha title url; do
  [ -n "$run_id" ] || continue
  if [ "$status" != "completed" ]; then
    continue
  fi

  fetch_jobs "$run_id"
  if run_has_required_full_jobs; then
    selected_id="$run_id"
    selected_created="$created"
    selected_status="$status"
    selected_conclusion="$conclusion"
    selected_sha="$sha"
    selected_title="$title"
    selected_url="$url"
    selected_full="yes"
    if run_full_jobs_green && [ "$conclusion" = "success" ]; then
      selected_green="yes"
    fi
    break
  fi
done <"$RUNS_TSV"

if [ "$selected_id" = "n/a" ]; then
  : >"$JOBS_TSV"
fi

now_utc="$(date -u +%FT%TZ)"
git_commit="$(git rev-parse --short HEAD 2>/dev/null || printf 'unknown')"

write_required_job_rows() {
  while IFS= read -r job_name; do
    [ -n "$job_name" ] || continue
    if [ -s "$JOBS_TSV" ]; then
      row="$(awk -F '\t' -v wanted="$job_name" '$1 == wanted { print $1 "\t" $2 "\t" $3 "\t" $4; found = 1; exit } END { if (!found) print wanted "\tmissing\tmissing\t" }' "$JOBS_TSV")"
    else
      row="$(printf '%s\tmissing\tmissing\t' "$job_name")"
    fi
    IFS="$tab" read -r name status conclusion url <<EOF_ROW
$row
EOF_ROW
    safe_name="$(printf '%s' "$name" | tr '|' '/')"
    if [ -n "$url" ]; then
      printf '| %s | `%s` | `%s` | [job](%s) |\n' "$safe_name" "$status" "$conclusion" "$url"
    else
      printf '| %s | `%s` | `%s` | n/a |\n' "$safe_name" "$status" "$conclusion"
    fi
  done <<EOF_JOBS
$REQUIRED_JOBS
EOF_JOBS
}

write_run_rows() {
  while IFS="$tab" read -r run_id created status conclusion sha title url; do
    safe_title="$(printf '%s' "$title" | tr '|' '/')"
    printf '| [%s](%s) | `%s` | `%s` | `%s` | `%s` | %s |\n' \
      "$run_id" "$url" "$created" "$status" "${conclusion:-n/a}" "${sha:-n/a}" "$safe_title"
  done <"$RUNS_TSV"
}

{
  echo "# Phase 2 Hosted Full CI Evidence"
  echo
  echo "This file is generated by \`scripts/record-phase2-hosted-full-ci-evidence.sh\`."
  echo
  echo "It checks whether recent hosted CI history contains a completed full CI run."
  echo "Normal push CI is useful, but it skips the expensive cross-host evidence jobs."
  echo
  echo "## Scope"
  echo
  echo "- Repository: \`$REPO\`"
  echo "- Branch: \`$BRANCH\`"
  echo "- Git commit at recording time: \`$git_commit\`"
  echo "- Generated at (UTC): \`$now_utc\`"
  echo "- Runs inspected: \`$LIMIT\`"
  if [ -n "$CREATED_FILTER" ]; then
    echo "- Created filter: \`$CREATED_FILTER\`"
  fi
  echo "- Require success: \`$REQUIRE_SUCCESS\`"
  echo
  echo "## Selected Full CI Run"
  echo
  echo "| Field | Value |"
  echo "|---|---|"
  echo "| Run | [$selected_id]($selected_url) |"
  echo "| Created | \`$selected_created\` |"
  echo "| Status | \`$selected_status\` |"
  echo "| Conclusion | \`$selected_conclusion\` |"
  echo "| Head SHA | \`$selected_sha\` |"
  echo "| Title | $selected_title |"
  echo "| Required full jobs present | \`$selected_full\` |"
  echo "| Required full jobs green | \`$selected_green\` |"
  echo
  echo "## Required Full Jobs"
  echo
  echo "| Job | Status | Conclusion | URL |"
  echo "|---|---|---|---|"
  write_required_job_rows
  echo
  echo "## Recent CI Runs Inspected"
  echo
  echo "| Run | Created | Status | Conclusion | Head SHA | Title |"
  echo "|---|---|---|---|---|---|"
  write_run_rows
  echo
  echo "## Reading This Report"
  echo
  echo "A passing report means the selected hosted CI run did not skip the full Phase 2"
  echo "evidence jobs and every required full job finished green. It is separate from"
  echo "normal fast CI stability, self-hosted full-gate proof, fuzz soak, and the"
  echo "outside walkthrough."
} >"$OUTPUT"

echo "wrote $OUTPUT"

if [ "$REQUIRE_SUCCESS" = "1" ] && [ "$selected_green" != "yes" ]; then
  echo "hosted full CI evidence error: no completed full CI run with all required jobs green was found" >&2
  exit 1
fi

exit 0
