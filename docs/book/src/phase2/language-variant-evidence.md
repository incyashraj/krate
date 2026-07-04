# Language Variant Evidence

This page explains how to record one evidence file for Phase 2 language
variants.

The goal is simple: keep one markdown report that shows fixture readiness and
test outcomes for Rust, Go, and TypeScript language paths.

## Run The Recorder

From the repo root:

```bash
scripts/record-phase2-language-variant-evidence.sh
```

Default output:

```text
target/phase2-language-variant-evidence/language-variant-evidence.md
```

You can pass a custom output path:

```bash
scripts/record-phase2-language-variant-evidence.sh \
  --output target/phase2-language-variant-evidence/macos-arm64.md
```

For three-host comparison after collecting Linux, macOS, and Windows reports:

```bash
scripts/compare-phase2-language-variant-evidence.sh \
  target/phase2-language-variant-evidence/linux.md \
  target/phase2-language-variant-evidence/macos.md \
  target/phase2-language-variant-evidence/windows.md
```

In hosted full CI, each OS lane uploads one language-variant evidence artifact
(`language-variant-evidence-<os>`). You can download those three files and run
the comparator locally to check the proof set.

Hosted full CI also runs this comparator automatically after the full matrix, so
drift is caught in CI as part of the same run.

## What It Records

The report includes:

- git commit, host OS, host architecture, and UTC timestamp
- fixture mode (`optional`, `any`, `both`, `go`, `ts`)
- exit codes for:
  - `scripts/build-phase2-language-variant-fixtures.sh`
  - `scripts/test-phase2-language-variants.sh`
- fixture presence and SHA-256 hashes for:
  - `krate_go_clock.wasm`
  - `krate_go_cat.wasm`
  - `krate_go_curl.wasm`
  - `krate_ts_clock.wasm`
  - `krate_ts_cat.wasm`
  - `krate_ts_curl.wasm`
- tail logs for build and test steps

## What The Comparator Checks

The comparator checks that:

- all three reports came from the same git commit
- the Linux, macOS, and Windows files are labelled as the right host
- the build and runtime test steps passed on every host
- each fixture row is present in every report
- a fixture is either present on all hosts or missing on all hosts
- every present fixture has a SHA-256 hash recorded

It does not require independently generated TypeScript fixtures to have the
same hash on every operating system. The useful Phase 2 proof is that the same
source fixtures build, pass import checks, and run through Krate on Linux,
macOS, and Windows. Byte-for-byte reproducible jco output is a different
promise and is not needed for this phase.

## Strict Mode

By default, the recorder writes a report even when one command fails.

For CI-style behavior, use strict mode:

```bash
scripts/record-phase2-language-variant-evidence.sh --strict
```

In strict mode, the script exits non-zero when build or test fails.

You can also choose a fixture mode directly:

```bash
scripts/record-phase2-language-variant-evidence.sh --mode ts --strict
```

## Why This Helps Phase 2

This gives one repeatable artifact for language-variant progress.

It does not replace cross-host sample evidence for Rust clock/cat/curl, but it
makes language-variant status and cross-host behavior easier to review before
Phase 2 exit.
