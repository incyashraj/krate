# UAPI Freeze Decision Packet

**Status:** Draft. UAPI v0.1 is not frozen yet.

This page is the decision packet for freezing the Phase 2 UAPI.

The freeze decision is small but serious. It says the `layer36:*@0.1.0`
contracts are stable enough for apps and SDKs to depend on them. It does not
mean Layer36 is complete. It only means this first CLI contract is no longer a
casual draft.

## Decision State

Decision: Not frozen yet.

Current recommendation: keep the UAPI as a freeze candidate while the last
Phase 2 proof work finishes.

The current WIT shape is strong enough for samples, docs, SDK smoke tests, and
cross-host evidence. The final freeze should wait until the exit review has a
fresh evidence bundle and the outside Rust walkthrough is filled.

## Scope

The freeze covers these Phase 2 packages:

- `layer36:io@0.1.0`
- `layer36:fs@0.1.0`
- `layer36:net@0.1.0`
- `layer36:time@0.1.0`
- `layer36:locale@0.1.0`

The freeze also covers the `layer36:platform/cli@0.1.0` world shape, because it
defines how CLI components import those packages and export `run`.

## What Must Be True

Before the decision changes from `Not frozen yet` to `Frozen`, the reviewer
should see:

- `scripts/check-uapi.sh` passes
- `scripts/check-uapi-freeze-lock.sh` passes
- `scripts/record-phase2-uapi-freeze-review.sh --strict` passes
- `scripts/record-phase2-exit-bundle.sh --strict` passes
- hosted CI is green for the freeze commit
- the self-hosted full gate is green for the freeze commit
- the timed Rust walkthrough packet is filled and accepted
- the Phase 2 exit ledger has no pending gate

The Go track may remain experimental for runtime parity in Phase 2. That is
allowed only because the decision is recorded in
[Go Phase 2 Decision](go-phase2-decision.md) and guarded by import-purity checks.

## No-Go Conditions

Do not freeze if any of these are true:

- a WIT file changed but the freeze lock was not regenerated
- generated reference docs are stale
- UCap deny-before-adapter coverage is missing for a current UAPI entry
- a sample app relies on direct host APIs instead of `layer36:*` imports, except
  for the documented experimental Go runtime path
- the outside walkthrough has not been completed
- CI or self-hosted evidence is failing on the freeze commit

## Reviewer Signoff

Fill this section during the final review.

| Field | Value |
|---|---|
| Reviewer | Pending |
| Freeze commit | Pending |
| Hosted CI run | Pending |
| Self-hosted full gate run | Pending |
| Exit bundle path or artifact | Pending |
| Walkthrough packet | Pending |
| Decision date | Pending |

## If Accepted Later

When the freeze is accepted:

1. Replace the decision line with the frozen wording.
2. Fill the reviewer signoff table.
3. Update the Phase 2 exit ledger gate `P2E-01`.
4. Record the change in `Plan/Phase-2-Plan.md`.
5. Add an ADR only if the final review changes a rule that future phases depend
   on.

After freeze, breaking changes need a new package version such as
`layer36:fs@0.2.0`. Additive changes can be considered only if they do not
change existing names, parameter order, result shape, capability meaning, or
error meaning.
