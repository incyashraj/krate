# UAPI Freeze Review

This page is the checklist we use before calling Phase 2 UAPI v0.1 frozen.

Freezing does not mean Layer36 is finished. It means the Phase 2 contract is
stable enough that apps can depend on it without us casually changing function
names, error shapes, resource behavior, or capability strings.

## Current State

The Phase 2 UAPI is a strong draft.

What is already in place:

- WIT packages exist for `io`, `fs`, `net`, `time`, and `locale`
- `wasm-tools` validates the world and each dependency package
- `check-uapi` checks package names, world shape, imports, naming, permission
  error variants, and WIT docs
- the generated reference is published in the book
- the current contract evidence snapshot is published in
  [UAPI Freeze Evidence](uapi-freeze-evidence.md)
- hosted CI and self-hosted CI fail if that evidence page is stale
- Rust sample apps use the current SDK facade
- TypeScript fixtures build through jco and pass local runtime checks
- Go TinyGo artifacts build, but are not runtime fixtures yet because they still
  import WASI host APIs

What is not frozen yet:

- the final v0.1 wording for every function and error case
- cross-host evidence for Linux, macOS, and Windows over a stable window
- final decision on whether Go is required for the formal Phase 2 exit or marked
  experimental for the first UAPI slice

## Freeze Rules

After freeze:

- no breaking changes inside `layer36:*@0.1.0`
- no removing functions, records, variants, or enum cases
- no changing parameter order or result shape
- no changing capability string meaning
- no changing error meaning in a way that breaks existing apps

If we need a breaking change later, we publish a new package version such as
`layer36:fs@0.2.0` beside the old one.

## Review Checklist

Before checking the Phase 2 WIT freeze box, all items below should be true.

### Contract Shape

- [ ] `scripts/check-uapi.sh` passes on a clean checkout
- [ ] `scripts/generate-uapi-freeze-evidence.sh` refreshes the published
      evidence page without manual edits
- [ ] generated UAPI reference is current
- [ ] every public WIT item has clear docs
- [ ] every function has a plain behavior note in the reference or the WIT docs
- [ ] every typed error case has a clear meaning
- [ ] default grants and explicit grants are documented

### Runtime Behavior

- [ ] every current UAPI entry reaches UCap before host adapter work
- [ ] denied calls fail before native file or network access
- [ ] file and stdio resources re-check grants on resource methods
- [ ] path normalization rules match policy matching rules
- [ ] HTTP URL endpoint parsing matches `net.connect` grant parsing

### Samples

- [ ] `layer36-clock` has deterministic fixed-time output
- [ ] `layer36-cat` reads granted files and denies missing grants
- [ ] `layer36-curl` fetches granted HTTP fixtures and denies missing grants
- [ ] sample components import only `layer36:*`
- [ ] Rust and TypeScript sample behavior matches for covered paths

### Language Tracks

- [ ] Rust SDK package smoke passes from the packaged crate
- [ ] Rust docs build without broken links
- [ ] TypeScript jco fixture build is reproducible
- [ ] Go TinyGo smoke artifacts build
- [ ] Go runtime fixture promotion decision is explicit:
      either import-pure runtime proof is ready, or Go is marked experimental for
      Phase 2 exit

### Evidence

- [ ] hosted CI is green after the freeze commit
- [ ] self-hosted full gate has a recent green run
- [ ] dependency audit is clean or has a documented temporary exception
- [ ] benchmark gate has a recent Phase 2 baseline check
- [ ] fuzz smoke has passed after the last WIT or parser change

## Commands

Run these from repo root:

```bash
scripts/check-uapi.sh
scripts/generate-uapi-freeze-evidence.sh
cargo test -p layer36-tools
env PATH="$HOME/.cargo/bin:$PATH" mdbook build docs/book
scripts/smoke-rust-sdk.sh
scripts/test-phase2-language-variants.sh
```

For local runner evidence:

```bash
gh workflow run self-hosted-ci.yml
```

For Go readiness:

```bash
scripts/build-phase2-go-variant-smoke.sh
LAYER36_GO_RUNTIME_FIXTURE_MODE=optional scripts/promote-phase2-go-runtime-fixtures.sh
```

The second command should only promote fixtures when all Go artifacts import
`layer36:*` APIs only.

## Freeze Decision

The freeze decision should be recorded in the Phase 2 plan and in an ADR if it
changes a rule that future phases depend on.

Until that happens, treat the current WIT as a serious draft: stable enough to
test and document, not stable enough to promise forever.
