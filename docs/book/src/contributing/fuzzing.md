# Fuzzing

Krate now has a first Phase 2 fuzz harness set in:

```text
fuzz/
```

Current targets:

- `manifest_parse` for `Manifest::parse`
- `logical_path_parse` for shared path normalization and operation intent checks
- `policy_match` for capability parse and session-policy matching behavior

## Install

```bash
cargo install cargo-fuzz --locked
rustup toolchain install nightly --profile minimal
```

## Run a short smoke

From repo root:

```bash
scripts/run-phase2-fuzz-smoke.sh
```

This runs each target for a short window to catch immediate crashes.

## Run longer sessions

```bash
cargo fuzz run manifest_parse -- -max_total_time=300
cargo fuzz run logical_path_parse -- -max_total_time=300
cargo fuzz run policy_match -- -max_total_time=300
```

For full Phase 2 exit work, we still need nightly multi-hour runs and trend
tracking. This page covers the first harness setup only.
