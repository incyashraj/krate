#!/usr/bin/env sh
# The CP1 exit test (Plan/Master-Plan-2026-09-05.md, "CP1 -- Identity,
# signing, and the claim gate"), clause by clause, as one command.
#
# Every clause names the proof it rests on -- a test that runs here, or a
# checker that reads the tree -- and the script says PASS or FAIL per
# clause and exits 1 if any clause failed. It runs the same things CI runs;
# what it adds is one place that says whether the checkpoint's exit test
# holds on THIS machine at THIS commit, so nobody has to remember which
# nine tests and four checkers make up the sentence in the plan.
#
#   sh scripts/cp1-exit-test.sh            # every clause
#   sh scripts/cp1-exit-test.sh --list     # the clauses and their proofs, without running
#
# Two things it deliberately does not do. It does not run the Windows lane:
# "on every host" is CI's to prove (the full lanes run nightly, on a
# [full-ci] push, or on `gh workflow run ci.yml -f full=true`), and
# scripts/release-decision.py reads those verdicts into the evidence
# registry. And it does not make the claim gate's material claims green:
# `evidence-registry.py publication` is reported as information, because a
# claim the registry cannot carry is meant to read UNSUPPORTED until real
# evidence is retained for it (IC-829), never to be waved through here.
set -u
ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$ROOT" || exit 2
. "$ROOT/scripts/rust-env.sh"
LOGS="$ROOT/target/cp1-exit-test"
mkdir -p "$LOGS"

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

case "${1:-}" in
  --list) LIST=1 ;;
  "") ;;
  *) echo "usage: sh scripts/cp1-exit-test.sh [--list]" >&2; exit 2 ;;
esac

clause "two bundles differing only in source/ have different editable digests and the same runnable digest" \
  "krate-cli: each_identity_moves_only_when_what_it_names_changes; krate-bundle: repacking_an_app_keeps_its_execution_identity_and_changes_the_file, a_fork_records_its_parent_in_the_project_identity_and_not_the_execution_one" \
  sh -c 'cargo test -q -p krate-cli --test cli -- each_identity_moves_only_when_what_it_names_changes && cargo test -q -p krate-bundle --lib -- repacking_an_app_keeps_its_execution_identity_and_changes_the_file a_fork_records_its_parent_in_the_project_identity_and_not_the_execution_one'

clause "the E3 namespace-theft probe fails against lineage-bound storage" \
  "krate-cli (bin): only_a_trustworthy_verdict_earns_a_publishers_storage, storage_follows_the_declared_id_when_nothing_verifies_it, rotating_a_release_key_keeps_the_publishers_storage, the_names_people_read_are_not_the_lineage_that_keys_the_data, group_storage_is_kept_away_from_private_storage" \
  cargo test -q -p krate-cli --bin krate -- only_a_trustworthy_verdict_earns_a_publishers_storage storage_follows_the_declared_id_when_nothing_verifies_it rotating_a_release_key_keeps_the_publishers_storage the_names_people_read_are_not_the_lineage_that_keys_the_data group_storage_is_kept_away_from_private_storage

clause "a signed release verifies on a clean machine offline, and fails verification after any single-byte mutation per the test vectors" \
  "krate-bundle: signing::vectors::every_single_byte_mutation_of_the_vectors_fails_to_verify; krate-cli: a_signed_app_that_was_changed_afterwards_is_refused, a_signed_app_reports_its_release_id_and_an_unsigned_one_reports_none, a_withdrawn_key_is_refused_only_for_what_it_signed_after_the_compromise" \
  sh -c 'cargo test -q -p krate-bundle --lib -- every_single_byte_mutation_of_the_vectors_fails_to_verify && cargo test -q -p krate-cli --test cli -- a_signed_app_that_was_changed_afterwards_is_refused a_signed_app_reports_its_release_id_and_an_unsigned_one_reports_none a_withdrawn_key_is_refused_only_for_what_it_signed_after_the_compromise'

clause "krate pack rebuilds byte-equal on a second machine, and an editable bundle rebuilds from what it carries" \
  "krate-bundle: pack_is_byte_equal_on_every_machine_this_suite_runs_on, the_archive_writer_depends_on_nothing_but_its_entries; krate-cli: an_editable_bundle_rebuilds_locked_from_what_it_carries (needs cargo-component; skips without it)" \
  sh -c 'cargo test -q -p krate-bundle --lib -- pack_is_byte_equal_on_every_machine_this_suite_runs_on the_archive_writer_depends_on_nothing_but_its_entries && cargo test -q -p krate-cli --test cli -- an_editable_bundle_rebuilds_locked_from_what_it_carries'

clause "every reader resolves one canonical record set, and the container profile refuses what it does not name" \
  "krate-cli: every_reader_resolves_one_record_set_and_the_extension_namespace_holds, every_adversarial_archive_is_refused_by_the_binary_people_run; scripts/krate-records.py --self-test" \
  sh -c 'python3 scripts/krate-records.py --self-test && cargo test -q -p krate-cli --test cli -- every_reader_resolves_one_record_set_and_the_extension_namespace_holds every_adversarial_archive_is_refused_by_the_binary_people_run'

clause "the claims generator refuses a sentence with no evidence record" \
  "scripts/evidence-registry.py --self-test and validate; scripts/check-claims.py --self-test" \
  sh -c 'python3 scripts/evidence-registry.py --self-test && python3 scripts/evidence-registry.py validate && python3 scripts/check-claims.py --self-test'

clause "the developer-control review covers every public boundary" \
  "scripts/developer-control-review.py --self-test and --check" \
  sh -c 'python3 scripts/developer-control-review.py --self-test && python3 scripts/developer-control-review.py --check'

clause "the licence map covers every component and dependency" \
  "scripts/licence-map.py --self-test and --check (reads cargo deny list)" \
  sh -c 'python3 scripts/licence-map.py --self-test && python3 scripts/licence-map.py --check'

if [ "${LIST:-0}" = 1 ]; then
  exit 0
fi

echo
echo "What the claim gate says about the material claims right now (information, not a clause):"
python3 scripts/evidence-registry.py publication 2>&1 | sed 's/^/    /'
echo
echo "What the public surfaces (and, on the founder's machine, Invest/) state without an evidence row:"
if python3 scripts/check-claims.py >"$LOGS/check-claims.log" 2>&1; then
  echo "    every stated figure names an evidence row"
else
  findings=$(grep -c "which no claim record vouches for\|was false" "$LOGS/check-claims.log")
  printf '    findings: %s (see %s/check-claims.log)\n' "$findings" "$LOGS"
fi
echo
if [ "$status" = 0 ]; then
  echo "CP1 exit test: every clause holds on this machine at $(git rev-parse --short HEAD 2>/dev/null || echo '?')."
else
  echo "CP1 exit test: at least one clause FAILED; the logs are under $LOGS."
fi
exit "$status"
