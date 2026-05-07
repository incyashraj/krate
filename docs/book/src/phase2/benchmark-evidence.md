# Benchmark Evidence

This page shows the repeatable way to record Phase 2 performance evidence for:

- startup path checks
- UAPI dispatch checks
- baseline regression checks

The goal is simple. We want proof that performance checks pass on Linux, macOS,
and Windows for the same commit.

## Record One Host Report

Run this on each host:

```bash
scripts/record-phase2-benchmark-evidence.sh --strict
```

This runs:

1. `cargo bench -p layer36-runtime --bench startup`
2. `cargo bench -p layer36-runtime --bench uapi_dispatch`
3. `scripts/check-benchmark-regression.sh`

Default output path:

`target/phase2-benchmark-evidence/benchmark-evidence.md`

Useful options:

```bash
scripts/record-phase2-benchmark-evidence.sh --strict --mode fail --threshold 10 --output /tmp/bench-linux.md
scripts/record-phase2-benchmark-evidence.sh --skip-bench --output /tmp/bench-reuse.md
```

## Compare Three Host Reports

After recording one report per host:

```bash
scripts/compare-phase2-benchmark-evidence.sh /tmp/bench-linux.md /tmp/bench-macos.md /tmp/bench-windows.md
```

The compare step checks:

- commit metadata matches across all three reports
- host label matches the expected OS lane
- startup, dispatch, and regression steps passed on all three reports
- required metric rows exist with current values
- baseline and threshold metadata is consistent across hosts

## Notes

- Runtime numbers are expected to differ across host hardware.
- This compare gate does not force numeric equality across hosts.
- It proves shape and pass state consistency for the same code revision.
