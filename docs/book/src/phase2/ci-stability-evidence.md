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

## What It Records

The report includes:

- repository and branch
- git commit at recording time
- latest hosted CI run
- latest Pages deploy run
- recent run history for both workflows
- completed success streak for each workflow

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

## Current Reading

The latest local report showed recent hosted CI and Pages runs green on `main`.
That is a good signal, but Phase 2 exit still needs the final UAPI candidate and
the remaining evidence bundle to line up.
