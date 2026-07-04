# Dependency Evidence

This page records how Phase 2 checks Rust dependency risk before exit.

The dependency audit is not only a CI checkbox. Krate puts Wasmtime,
Cranelift, parser crates, SDK helpers, and test tools in the trusted build
path. If one of those dependencies brings an unsafe license, an unexpected
source, or a known advisory, we need to see it before freezing the phase.

## What We Check

The local command is:

```bash
scripts/record-phase2-dependency-evidence.sh --strict
```

It runs:

```bash
scripts/check-dependencies.sh
```

That wrapper checks:

- advisories
- licenses
- banned crates and duplicate rules
- allowed source registries

The wrapper is deliberately plain. It keeps licenses, bans, and sources as hard
gates. Advisory database parser or lock-path failures are written as warnings
because current `cargo-deny` and RustSec advisory data can drift before the tool
is updated.

## Output

Default output path:

```text
target/phase2-dependency-evidence/dependency-evidence.md
```

The report includes:

- git commit
- host and architecture
- tool versions
- dependency audit exit code
- whether advisories were checked or skipped with a warning
- whether licenses, bans, and sources passed
- the tail of the dependency log

## How To Use It For Exit

For Phase 2 exit, keep one dependency evidence report for the final commit.

The preferred result is:

- dependency audit passed
- advisories checked
- licenses, bans, and sources passed

If a local machine cannot take the advisory database lock, use the hosted CI
audit as the final advisory proof and keep the local report as supporting
evidence. That keeps the review honest without blocking on a local cache path.

## Commands

```bash
scripts/record-phase2-dependency-evidence.sh --strict
scripts/check-dependencies.sh
```
