# Benchmark Evidence

This page shows the repeatable way to record Phase 2 performance evidence for:

- startup path checks
- full external `layer36 run` startup checks
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
4. `cargo build -p layer36-cli --release`
5. `scripts/build-layer36-clock-component.sh`
6. a measured external `layer36 run` loop for `layer36-clock`

Default output path:

`target/phase2-benchmark-evidence/benchmark-evidence.md`

Useful options:

```bash
scripts/record-phase2-benchmark-evidence.sh --strict --mode fail --threshold 10 --output /tmp/bench-linux.md
scripts/record-phase2-benchmark-evidence.sh --skip-bench --output /tmp/bench-reuse.md
scripts/record-phase2-benchmark-evidence.sh --skip-cli-startup --output /tmp/bench-no-cli.md
```

The full CLI startup check is intentionally separate from the Criterion
runtime benchmarks. It measures the real command path: process start, CLI
argument parsing, manifest loading, grant resolution, runtime setup, component
execution, and stdout collection.

## Compare Three Host Reports

After recording one report per host:

```bash
scripts/compare-phase2-benchmark-evidence.sh /tmp/bench-linux.md /tmp/bench-macos.md /tmp/bench-windows.md
```

The compare step checks:

- commit metadata matches across all three reports
- host label matches the expected OS lane
- startup, dispatch, and regression steps passed on all three reports
- full external CLI startup evidence passed on all three reports
- required metric rows exist with current values
- baseline and threshold metadata is consistent across hosts
- each metric stays within its baseline threshold on each host report

## Notes

- Runtime numbers are expected to differ across host hardware.
- This compare gate does not force numeric equality across hosts.
- It does enforce per-host threshold bounds from the recorded baseline table.
- It proves shape and pass state consistency for the same code revision.
- The full CLI startup report is evidence, not a strict numeric threshold yet.
  We should set a threshold only after Linux, macOS, and Windows reports are
  collected for the same commit.
