# Exit Bundle

The exit bundle is a local review report for Phase 2.

It does not say Phase 2 is finished. It collects the checks we want to see
green before a final exit review:

- UAPI contract shape
- UAPI freeze lock
- UAPI freeze decision packet
- adapter boundary shape
- exit ledger coverage
- closeout docs draft guard
- docs build
- dependency evidence
- Go readiness evidence
- optional hosted CI and Pages stability evidence
- optional self-hosted full-gate evidence
- optional Rust SDK package evidence

This is useful because Phase 2 now has many separate proof files. The bundle
puts the basic review state in one place.

## Record A Quick Bundle

Run this from the repo root:

```bash
scripts/record-phase2-exit-bundle.sh --strict
```

Default output path:

```text
target/phase2-exit-bundle/exit-bundle.md
```

Choose a custom output path:

```bash
scripts/record-phase2-exit-bundle.sh --strict --output /tmp/layer36-exit-bundle.md
```

## Include Rust SDK Proof

The Rust SDK package proof can touch the crates.io index, so it is optional in
the bundle. Include it when you want a fuller local review report:

```bash
scripts/record-phase2-exit-bundle.sh --strict --include-rust-sdk
```

Normal hosted CI already uploads the Rust SDK report as `rust-sdk-evidence`.

## Include Hosted CI Stability Proof

Hosted CI stability uses GitHub CLI, so it is optional in the bundle. Include it
when you are preparing a final review packet and have a logged-in `gh` session:

```bash
scripts/record-phase2-exit-bundle.sh --strict --include-ci-stability
```

This adds the recent hosted CI and GitHub Pages run history from
`scripts/record-phase2-ci-stability-evidence.sh`. The bundle calls that
recorder with `--require-success`, so a strict review bundle fails if hosted CI
or GitHub Pages does not show a completed green run.

For the final candidate, set `LAYER36_CI_STABILITY_CREATED` to the review
window you want, for example `>=2026-05-18`, before running the bundle. That
keeps older green hosted runs from being mistaken for final proof.

## Include Self-Hosted Full-Gate Proof

The self-hosted evidence recorder uses GitHub CLI too. Include it when the
local runner has produced a recent full-gate run:

```bash
scripts/record-phase2-exit-bundle.sh --strict --include-self-hosted
```

This adds recent `Self-hosted CI` run history from
`scripts/record-phase2-self-hosted-evidence.sh`. The bundle calls that recorder
with `--require-success`, so a strict review bundle fails if the inspected
self-hosted history does not contain a completed green full-gate run.

For the final candidate, set `LAYER36_SELF_HOSTED_CREATED` to the review window
you want, for example `>=2026-05-18`, before running the bundle. That keeps old
green local runs from being mistaken for final proof.

Dependency evidence is included by default because it is one of the final Phase
2 signoff checks. If local advisory lookup is blocked by a cache lock, the
bundle records that warning and still shows whether licenses, bans, and sources
passed.

Go readiness evidence is also included by default, but it is not treated as a
completion stamp. It records whether the TinyGo smoke artifacts build and
whether they are Layer36 import-pure. Today this helps reviewers see the exact
reason Go runtime fixtures are still blocked.

## What The Bundle Shows

The report includes:

- host and commit metadata
- pass or fail status for each included command
- the freeze candidate lock check result
- the UAPI freeze decision packet result
- the closeout docs draft guard result
- the dependency audit evidence result
- the Go readiness result and current import-purity status
- the hosted CI stability result when included
- the self-hosted full-gate result when included
- the current `P2E-*` gate snapshot from the exit ledger
- the current working tree state
- short log tails for each check

The bundle is meant for review and handoff. It should make a new session or a
human reviewer understand the current Phase 2 proof state without opening every
separate log first.
