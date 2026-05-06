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
scripts/record-phase2-language-variant-evidence.sh target/phase2-language-variant-evidence/macos-arm64.md
```

## What It Records

The report includes:

- git commit, host OS, host architecture, and UTC timestamp
- fixture mode (`optional`, `any`, `both`, `go`, `ts`)
- exit codes for:
  - `scripts/build-phase2-language-variant-fixtures.sh`
  - `scripts/test-phase2-language-variants.sh`
- fixture presence and SHA-256 hashes for:
  - `layer36_go_clock.wasm`
  - `layer36_go_cat.wasm`
  - `layer36_go_curl.wasm`
  - `layer36_ts_clock.wasm`
  - `layer36_ts_cat.wasm`
  - `layer36_ts_curl.wasm`
- tail logs for build and test steps

## Strict Mode

By default, the recorder writes a report even when one command fails.

For CI-style behavior, use strict mode:

```bash
LAYER36_LANGUAGE_VARIANT_EVIDENCE_STRICT=1 \
  scripts/record-phase2-language-variant-evidence.sh
```

In strict mode, the script exits non-zero when build or test fails.

## Why This Helps Phase 2

This gives one repeatable artifact for language-variant progress.

It does not replace cross-host sample evidence for Rust clock/cat/curl, but it
makes language-variant status and drift easier to review before Phase 2 exit.
