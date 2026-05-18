# CI Stability Evidence

Phase 2 needs more than one green run before exit.

This page explains the recorder we use to capture hosted CI and GitHub Pages
history in one reviewable file.

```bash
scripts/record-phase2-ci-stability-evidence.sh
```

Default output:

```text
target/phase2-ci-stability-evidence/ci-stability-evidence.md
```

For final exit review, require both hosted workflows to have at least one
completed green run in the inspected history:

```bash
scripts/record-phase2-ci-stability-evidence.sh --require-success
```

When the final candidate commit is ready, narrow the report to the review window
too:

```bash
scripts/record-phase2-ci-stability-evidence.sh --created '>=2026-05-18' --require-success
```

You can also include this report in the Phase 2 exit bundle:

```bash
scripts/record-phase2-exit-bundle.sh --strict --include-ci-stability
```

The exit bundle uses the stricter success check. If hosted CI or GitHub Pages
does not show a completed green run, the strict bundle fails.

## What It Records

The report includes:

- repository and branch
- git commit at recording time
- optional GitHub creation-date filter
- latest hosted CI run
- latest Pages deploy run
- recent run history for both workflows
- completed success streak for each workflow
- required success streak when strict checking is enabled

It uses GitHub CLI, so it needs a logged-in `gh` session with repository access.

## Why This Helps Phase 2

The Phase 2 exit criteria include CI stability over time. A screenshot or a
memory of green checks is not enough.

This recorder turns the GitHub run list into a plain markdown artifact. That
lets us attach a concrete report to the final Phase 2 review.

## What It Does Not Prove

This is hosted CI and Pages evidence only.

It does not replace:

- self-hosted full gate evidence
- long fuzz soak evidence
- benchmark evidence
- dependency evidence
- cross-host sample and adapter evidence

Those stay as separate proof tracks because each one answers a different
question.

Self-hosted run history is recorded with
`scripts/record-phase2-self-hosted-evidence.sh`.

## Current Reading

The latest local report showed recent hosted CI and Pages runs green on `main`.
That is a good signal, but Phase 2 exit still needs the final UAPI candidate and
the remaining evidence bundle to line up.
