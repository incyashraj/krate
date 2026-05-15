# Exit Bundle

The exit bundle is a local review report for Phase 2.

It does not say Phase 2 is finished. It collects the checks we want to see
green before a final exit review:

- UAPI contract shape
- UAPI freeze lock
- adapter boundary shape
- exit ledger coverage
- docs build
- dependency evidence
- Go readiness evidence
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
- the dependency audit evidence result
- the Go readiness result and current import-purity status
- the current `P2E-*` gate snapshot from the exit ledger
- the current working tree state
- short log tails for each check

The bundle is meant for review and handoff. It should make a new session or a
human reviewer understand the current Phase 2 proof state without opening every
separate log first.
