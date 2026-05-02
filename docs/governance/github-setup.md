# GitHub Setup Checklist

These settings require repository owner access and cannot be completed by local
file edits.

## Repository Metadata

- Description: `Layer36: universal app runtime and UAPI for native cross-platform software.`
- Website: GitHub Pages URL after docs deploy.
- Topics: `webassembly`, `wasm`, `cross-platform`, `runtime`, `rust`.
- Social preview: simple title card once final naming is resolved.

## Branch Protection For `main`

Enable:

- Require pull request before merging.
- Require at least one approving review.
- Require status checks to pass.
- Require branches to be up to date before merge.
- Block force pushes.
- Block deletions.

Required checks:

- `Format (rustfmt)`
- `Lint (clippy)`
- `Test (ubuntu-latest)`
- `Test (macos-latest)`
- `Test (windows-latest)`
- `Docs (mdBook)`
- `Dependency audit (cargo-deny)`

## Pages

- Source: GitHub Actions.
- Workflow: `.github/workflows/pages.yml`.
- The Pages workflow is intentionally manual (`workflow_dispatch`) until the
  repository setting above is enabled. Normal pushes still build docs through
  CI, but they do not attempt to deploy Pages before GitHub is ready.
- After first deploy, copy the live URL into `README.md`,
  `docs/book/book.toml`, and the repository website field.

## Labels

Apply `.github/labels.yml` manually or with a label-sync tool.

## Phase 0 Exit Notes

Record the date each external setting is completed in
`docs/book/src/phases/phase-0-status.md`.
