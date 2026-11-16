#!/usr/bin/env sh
# The CP0 exit test (Plan/Master-Plan-2026-09-05.md, "CP0 -- Stop the
# bleeding"), clause by clause, as one command.
#
#   "every item above has a landed commit plus a regression test; cargo
#   deny, fmt, clippy, and the fuzz smoke run green on main; the live Hub
#   returns no personal pricing; a re-run of the E2/E3/E4 exploit probes
#   (name traversal, wrapper injection, VACUUM bypass, namespace theft,
#   auto-grant paths, webhook failure) shows every operation refused."
#
# Each probe re-runs the regression test that closed it, and the clause
# counts the tests that actually ran: a cargo filter that matches nothing
# exits 0, and a renamed test must not turn a probe into a silent pass.
#
#   sh scripts/cp0-exit-test.sh            # every clause
#   sh scripts/cp0-exit-test.sh --list     # the clauses and their proofs, without running
#
# Network: the fuzz clause asks GitHub for the last successful nightly, and
# the pricing clause asks the live hub. Both say so when they cannot reach it.
set -u
ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$ROOT" || exit 2
. "$ROOT/scripts/rust-env.sh"
LOGS="$ROOT/target/cp0-exit-test"
mkdir -p "$LOGS"
HUB="${KRATE_HUB:-https://hub.krate.tech}"

status=0
n=0

clause() {
  # clause "<text>" "<proof>" <command...>
  text=$1; proof=$2; shift 2
  n=$((n + 1))
  log="$LOGS/clause-$n.log"
  if [ "${LIST:-0}" = 1 ]; then
    printf '%2d. %s\n      proof: %s\n' "$n" "$text" "$proof"
    return 0
  fi
  printf '%2d. %s\n' "$n" "$text"
  if "$@" >"$log" 2>&1; then
    printf '    PASS  %s\n' "$proof"
  else
    printf '    FAIL  %s\n          see %s\n' "$proof" "$log"
    status=1
  fi
}

# tests <expected count> <cargo test args...>: the run passes AND exactly
# that many tests ran.
tests() {
  want=$1; shift
  out="$(cargo test "$@" 2>&1)"; code=$?
  printf '%s\n' "$out"
  ran=$(printf '%s\n' "$out" | sed -n 's/^test result: ok\. \([0-9]*\) passed.*/\1/p' | awk '{s+=$1} END {print s+0}')
  [ "$code" -eq 0 ] || return 1
  if [ "$ran" -ne "$want" ]; then
    echo "expected $want tests to run, $ran did: a probe test was renamed or removed"
    return 1
  fi
}

fuzz_fresh() {
  since=$(python3 -c 'import datetime as d; print((d.datetime.now(d.timezone.utc)-d.timedelta(days=7)).strftime("%Y-%m-%dT%H:%M:%SZ"))')
  last=$(gh api "repos/incyashraj/krate/actions/workflows/self-hosted-fuzz-nightly.yml/runs?status=success&created=%3E%3D${since}&per_page=100" \
    --jq '[.workflow_runs[].created_at] | max // empty') || { echo "could not reach GitHub"; return 1; }
  [ -n "$last" ] || { echo "no successful fuzz nightly since $since"; return 1; }
  echo "last successful fuzz nightly: $last"
}

no_pricing() {
  # /billing answers 410 with the words, so the code is read, not failed on.
  page=$(curl -sS --max-time 20 -w '\n%{http_code}' "$HUB/billing") || { echo "could not reach $HUB"; return 1; }
  echo "$page"
  [ "$(echo "$page" | tail -n 1)" = 410 ] || return 1
  echo "$page" | grep -q "no paid plans" || return 1
  for door in /billing/config /billing/webhook /billing/checkout; do
    code=$(curl -s -o /dev/null --max-time 20 -w '%{http_code}' -X POST \
      -H 'content-type: application/json' -d '{}' "$HUB$door")
    echo "POST $door -> $code"
    [ "$code" = 410 ] || return 1
  done
}

case "${1:-}" in
  --list) LIST=1 ;;
  "") ;;
  *) echo "usage: sh scripts/cp0-exit-test.sh [--list]" >&2; exit 2 ;;
esac

clause "fmt is clean" \
  "cargo fmt --all -- --check" \
  cargo fmt --all -- --check

clause "clippy is clean" \
  "cargo clippy --workspace --all-targets --all-features -- -D warnings" \
  cargo clippy --workspace --all-targets --all-features -- -D warnings

clause "cargo deny: no advisory, licence, ban or source failure in either shipped lock" \
  "scripts/check-dependencies.sh (root workspace and studio/)" \
  sh scripts/check-dependencies.sh

clause "the fuzz nightly passed within the last 7 days" \
  "GitHub: a successful Fuzz Nightly run since seven days ago (as ci.yml's freshness job asks)" \
  fuzz_fresh

clause "the live hub returns no personal pricing" \
  "$HUB/billing says there are no paid plans; /billing/config, /billing/webhook and /billing/checkout answer 410" \
  no_pricing

clause "name traversal: a manifest name cannot reach outside the folder it names" \
  "krate-cli (bin) safe_path_name_tests: traversal_cannot_survive_a_safe_name, a_name_with_nothing_usable_becomes_app (IC-217)" \
  tests 2 -q -p krate-cli --bin krate -- traversal_cannot_survive_a_safe_name a_name_with_nothing_usable_becomes_app

clause "wrapper injection: a manifest name cannot add a command to a wrapper, and no wrapper pipes the internet into a shell" \
  "krate-cli (bin) script_safety_tests: a_manifest_name_cannot_add_a_line_to_a_wrapper, shell_metacharacters_do_not_survive, no_wrapper_pipes_the_internet_into_a_shell (IC-380, IC-381)" \
  tests 3 -q -p krate-cli --bin krate -- a_manifest_name_cannot_add_a_line_to_a_wrapper shell_metacharacters_do_not_survive no_wrapper_pipes_the_internet_into_a_shell

clause "VACUUM bypass: no spelling of VACUUM INTO, ATTACH or a file pragma leaves the app's database" \
  "krate-runtime sql_host: whitespace_cannot_smuggle_a_vacuum_past_the_filter, the_authorizer_refuses_attach_however_it_is_written, attaching_another_database_is_refused, pragmas_and_file_functions_are_refused (IC-248)" \
  tests 4 -q -p krate-runtime --lib -- whitespace_cannot_smuggle_a_vacuum_past_the_filter the_authorizer_refuses_attach_however_it_is_written attaching_another_database_is_refused pragmas_and_file_functions_are_refused

clause "namespace theft: an app claiming another's id reads none of its data or grants" \
  "krate-cli (bin): a_verified_app_is_isolated_from_anyone_claiming_its_id, only_a_trustworthy_verdict_earns_a_publishers_storage, a_remembered_grant_is_never_inherited_by_whoever_claims_the_id (IC-736, IC-017)" \
  tests 3 -q -p krate-cli --bin krate -- a_verified_app_is_isolated_from_anyone_claiming_its_id only_a_trustworthy_verdict_earns_a_publishers_storage a_remembered_grant_is_never_inherited_by_whoever_claims_the_id

clause "auto-grant paths: a preview or screenshot run is granted nothing the app did not declare, and reaches no network" \
  "krate-policy: a_screenshot_run_cannot_reach_the_world, a_screenshot_run_is_never_wider_than_what_was_declared (IC-841, IC-219)" \
  tests 2 -q -p krate-policy --lib -- a_screenshot_run_cannot_reach_the_world a_screenshot_run_is_never_wider_than_what_was_declared

clause "webhook failure: every payment door answers 410 and nothing reaches Stripe" \
  "cloud/worker/test/no-paid-plans.test.mjs (IC-409, superseded by F-322)" \
  sh -c 'cd cloud/worker && node --experimental-wasm-modules --test test/no-paid-plans.test.mjs'

if [ -f "$ROOT/Plan/ledger/validate.py" ]; then
  clause "every CP0 item is closed with a commit, tests and a reproduced result" \
    "Plan/ledger/validate.py (private; present on the founder's machine only)" \
    python3 "$ROOT/Plan/ledger/validate.py"
fi

[ "${LIST:-0}" = 1 ] && exit 0
echo
if [ "$status" -eq 0 ]; then
  echo "CP0 exit test: every clause holds at $(git rev-parse --short HEAD)."
else
  echo "CP0 exit test: NOT met at $(git rev-parse --short HEAD). Logs: $LOGS"
fi
exit "$status"
