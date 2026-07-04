# GitHub Setup Checklist

These settings require repository owner access and cannot be completed by local
file edits.

## Repository Metadata

- Description: configured on 2026-05-02.
- Website: `https://incyashraj.github.io/layer6x6/`.
- Topics: `webassembly`, `wasm`, `cross-platform`, `runtime`, `rust`.
- Social preview: simple title card once final naming is resolved.
- Visibility: currently private; make public when ready for external readers.

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
- `Docs (mdBook)`

Do not require the expensive full-matrix checks while the repository is using
the included GitHub Actions minutes. The full matrix is still available, but it
is intentionally manual.

Full validation checks:

- `Full test (ubuntu-latest)`
- `Full test (macos-latest)`
- `Full test (windows-latest)`
- `Benchmarks (warning only)`
- `Dependency audit (cargo-deny)`

Run full validation from **Actions -> CI -> Run workflow -> full = true**, or by
including `[full-ci]` in a push commit message. Use it before releases,
architecture-changing runtime work, and any change that touches host-specific
behavior.

## Pages

- Source: GitHub Actions.
- Workflow: `.github/workflows/pages.yml`.
- Live URL: `https://incyashraj.github.io/layer6x6/`.
- The Pages workflow is intentionally manual (`workflow_dispatch`) until the
  repository setting above is enabled. Normal pushes still build docs through
  CI, but they do not attempt to deploy Pages before GitHub is ready.
- After first deploy, copy the live URL into `README.md`,
  `docs/book/book.toml`, and the repository website field.

## Labels

Applied with GitHub CLI on 2026-05-02 from `.github/labels.yml`.

The default `good first issue` label already existed on GitHub and was updated
in place with the Krate description.

## Issues

Opened with GitHub CLI on 2026-05-02:

- `#2` `ci(p0): verify mdBook build on pull requests`
- `#3` `docs(p0): review setup on a fresh machine`
- `#4` `docs(p0): fill out the trademark search log`
- `#5` `docs(p0): proofread the README for first-time readers`
- `#6` `phase 1: prove one WASM component runs on three desktop hosts`
- `#7` `docs(p0): add screenshots to the first-PR guide`

## Phase 0 Exit Notes

Record the date each external setting is completed in
`docs/book/src/phases/phase-0-status.md`.
