# Rust SDK Evidence

This page explains how we record Phase 2 evidence for the Rust SDK.

The Rust SDK is not published yet. Until UAPI v0.1 is frozen, the useful proof
is:

- the crate can be packaged
- the package contains the public files a developer needs
- an outside-workspace app can compile against that package
- the SDK docs build without pulling in unrelated workspace docs

## Record One Evidence Report

Run this from the repo root:

```bash
scripts/record-phase2-rust-sdk-evidence.sh --strict
```

Default output path:

```text
target/phase2-rust-sdk-evidence/rust-sdk-evidence.md
```

Choose a custom output path:

```bash
scripts/record-phase2-rust-sdk-evidence.sh --strict --output /tmp/rust-sdk-evidence.md
```

## What It Checks

The recorder runs:

1. `scripts/smoke-rust-sdk.sh`
2. `cargo doc -p krate --no-deps`

It also records whether the packaged SDK contains:

- `Cargo.toml`
- `README.md`
- `src/lib.rs`
- `src/bindings.rs`

## Hosted CI Evidence

Normal hosted CI now uploads:

- `rust-sdk-evidence`

That artifact gives `P2E-03` a concrete proof source while crates.io publishing
stays blocked on the final UAPI freeze decision.
