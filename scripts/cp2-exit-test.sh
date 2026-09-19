#!/usr/bin/env sh
# The CP2 exit test (Plan/Master-Plan-2026-09-05.md, "CP2 -- Recipient
# safety: capability broker, runtime limits, one lifecycle"), clause by
# clause, as one command.
#
# CP2's sentence is "what the recipient approved is what the app can do, on
# every route, with real resource bounds and honest results". Its exit test
# spells that out:
#
#   the portable capability suite passes on all three desktops: denial at
#   every phase uses the same exit class; a revoked grant stops mid-run;
#   the picker opens the actual picked file and nothing else; an
#   undeclared-audio bundle is refused; a fuel-exhausted and a wall-clock-
#   stuck app both terminate with the documented codes on every route; the
#   E2 adversarial corpus still passes. Application-data export/delete and
#   a failed schema migration also pass without crossing identities or
#   destroying the previous usable state.
#
# Same shape as scripts/cp1-exit-test.sh: every clause names the proof it
# rests on, the script says PASS or FAIL per clause, and it exits 1 if any
# clause failed.
#
#   sh scripts/cp2-exit-test.sh            # every clause
#   sh scripts/cp2-exit-test.sh --list     # the clauses and their proofs
#
# WHAT THIS SCRIPT WILL NOT DO
#
# It will not print PASS for a clause that has no proof behind it. A
# checkpoint harness whose green means "nobody wrote a test" is worse than
# no harness: it converts an open question into a false answer, and CP1's
# clause 4 sat red for weeks precisely because its harness was honest.
#
# So clauses with no proof yet report TODO and are counted separately. TODO
# does not fail the run -- CP2 is in progress and a harness that is red
# from the first day gets ignored -- but the script refuses to say the exit
# test HOLDS while any TODO remains, and prints what is missing.
#
# Like CP1's, it does not run the Windows or Linux lanes. "On all three
# desktops" is CI's to prove; this says whether the clause holds on THIS
# machine at THIS commit.
set -u
ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$ROOT" || exit 2
. "$ROOT/scripts/rust-env.sh"
LOGS="$ROOT/target/cp2-exit-test"
mkdir -p "$LOGS"

status=0
todos=0
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

todo() {
  # todo "<text>" "<what is missing>"
  text=$1; missing=$2
  n=$((n + 1))
  todos=$((todos + 1))
  if [ "${LIST:-0}" = 1 ]; then
    printf '%2d. %s\n      TODO: %s\n' "$n" "$text" "$missing"
    return 0
  fi
  printf '%2d. %s\n' "$n" "$text"
  printf '    TODO  %s\n' "$missing"
}

if [ "${1:-}" = "--list" ]; then
  LIST=1
fi

cargo_test() {
  # cargo test with the package and filter given, failing if the filter
  # matched NOTHING. `cargo test some_name_that_does_not_exist` exits 0,
  # so a clause naming a renamed or deleted test would otherwise pass
  # while proving nothing -- the failure mode this whole script exists to
  # avoid.
  pkg=$1; filter=$2; shift 2
  out=$(cargo test -p "$pkg" "$@" "$filter" 2>&1) || {
    printf '%s\n' "$out"
    return 1
  }
  printf '%s\n' "$out"
  printf '%s\n' "$out" | grep -qE '^test .* \.\.\. ok' || {
    printf '\nNO TEST MATCHED "%s" in %s -- the clause names a test that does not exist.\n' \
      "$filter" "$pkg"
    return 1
  }
}

clause \
  "a revoked grant stops working everywhere, not only where a reason is carried" \
  "krate-policy: a_revoked_grant_stops_working_everywhere_not_just_in_decide, an_expired_grant_is_reported_as_expired_and_not_as_revoked" \
  cargo_test krate-policy a_revoked_grant_stops_working_everywhere_not_just_in_decide

clause \
  "the two gates never disagree: check and decide answer the same question" \
  "krate-runtime: the_reasoned_gate_and_the_enforcing_gate_always_agree" \
  cargo_test krate-runtime the_reasoned_gate_and_the_enforcing_gate_always_agree

clause \
  "the picker opens the actual picked file and nothing else" \
  "krate-runtime: a_chosen_file_that_is_a_symlink_opens_nothing, a_chosen_file_does_not_go_through_the_sandboxing_open, a_file_the_person_granted_reads_without_any_fs_grant; chosen_files (6 tests: an invented token opens nothing, a token from another run resolves to nothing, a token carries nothing about the path)" \
  cargo_test krate-runtime chosen_file

clause \
  "a token names one file and cannot be guessed, reused across runs, or read for a path" \
  "krate-runtime: chosen_files::tests" \
  cargo_test krate-runtime chosen_files::

clause \
  "a capability the runtime cannot honour is not declarable, so no hollow promise reaches a consent sheet" \
  "krate-manifest: a_capability_the_runtime_cannot_honour_is_not_declarable (asserts the RULE, not one example)" \
  cargo_test krate-manifest a_capability_the_runtime_cannot_honour_is_not_declarable

clause \
  "an undeclared capability is refused at dispatch, including one with no call to make" \
  "krate-runtime: a_drop_needs_ui_dropzone_like_every_other_capability -- a drop ARRIVES rather than being asked for, so it is the case a grant check is easiest to forget (K-420)" \
  cargo_test krate-runtime a_drop_needs_ui_dropzone_like_every_other_capability

clause \
  "a default grant covers what it says and nothing privileged" \
  "krate-policy: default_dialog_grants_cover_message_boxes_and_nothing_privileged" \
  cargo_test krate-policy default_dialog_grants_cover_message_boxes_and_nothing_privileged

clause \
  "a fuel-exhausted app terminates with the documented code" \
  "krate-cli: fuel_limit_exits_with_limit_code, a_tiny_fuel_budget_stops_the_run_with_limit_exceeded" \
  cargo_test krate-cli fuel_limit_exits_with_limit_code --test cli

clause \
  "running out of guest memory is survivable rather than an uncatchable trap" \
  "krate-runtime: the_memory_limiter_refuses_rather_than_trapping (K-395); krate: mem_tests -- the guest half, so an app can ask before allocating instead of aborting (K-416)" \
  cargo_test krate-runtime the_memory_limiter_refuses_rather_than_trapping

clause \
  "the E2 adversarial corpus is refused by the binary people actually run" \
  "krate-cli: every_adversarial_archive_is_refused_by_the_binary_people_run" \
  cargo_test krate-cli every_adversarial_archive_is_refused_by_the_binary_people_run --test cli

clause \
  "a denial says how to grant it and names what was run" \
  "krate-cli: a_denial_tells_you_how_to_grant_and_names_what_you_ran" \
  cargo_test krate-cli a_denial_tells_you_how_to_grant_and_names_what_you_ran --test cli

clause \
  "run --json reports denied capabilities before running a single instruction" \
  "krate-cli: run_json_reports_denied_capabilities_before_running" \
  cargo_test krate-cli run_json_reports_denied_capabilities_before_running --test cli

todo \
  "denial at every phase uses the same exit class" \
  "no test compares the exit code of a denial across phase 1, 2, 3 and 4. \
CheckStage::exit_code maps check-app STAGES (10-16); a denial's class is a \
different number and nothing pins it. Needs one test that denies the same \
capability on every route and asserts one code."

todo \
  "a wall-clock-stuck app terminates with the documented code on every route" \
  "the outer supervisor named in CP2 does not exist yet: nothing kills a \
non-cooperative run on wall-clock. A guest that never returns is killed by \
whatever ran it (check-app's 60s), which is not the same promise and is not \
the same code on every route."

todo \
  "an undeclared-audio bundle is refused" \
  "audio is declarable and enforced, but no test packs a bundle that CALLS \
audio without declaring it and asserts the refusal. The dropzone clause above \
is the same shape and does exist; this one is the audio instance of it."

todo \
  "application-data export and delete do not cross identities" \
  "the application-data contract (local-only, personal sync, shared, \
organization profiles) is designed in the plan and not built. Nothing to \
test yet."

todo \
  "a failed schema migration keeps the previous usable state" \
  "same: application data is not versioned independently of the runnable \
file yet, so there is no migration to fail."

if [ "${LIST:-0}" = 1 ]; then
  exit 0
fi

sha=$(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo unknown)
printf '\n'
if [ "$status" -ne 0 ]; then
  printf 'CP2 exit test: at least one clause FAILED; the logs are under %s.\n' "$LOGS"
  exit 1
fi
if [ "$todos" -ne 0 ]; then
  printf 'CP2 exit test: every clause with a proof holds on this machine at %s,\n' "$sha"
  printf 'but %d of %d clauses have no proof yet (TODO above). The exit test does\n' "$todos" "$n"
  printf 'NOT hold until those exist -- a checkpoint is not passed by the clauses\n'
  printf 'somebody got round to writing.\n'
  exit 0
fi
printf 'CP2 exit test: every clause holds on this machine at %s.\n' "$sha"
