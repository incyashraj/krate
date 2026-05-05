# Phase 2 Benchmarks

Phase 2 adds UAPI, so the runtime does more work than Phase 1. Every app call
now passes through a policy check before the host adapter touches files,
network, time, locale, or streams. That check needs to be cheap.

These numbers are an early local read, not a release promise. They tell us
whether the current design is in the right range before we freeze UAPI v0.1.

## Reference Machine (Current Baseline)

| Field | Value |
|---|---|
| Date | 2026-05-05 |
| CPU | Apple M4 |
| OS | macOS |
| Architecture | arm64 |
| Rust | rustc 1.91.1 |

## Commands

```bash
cargo bench -p layer36-runtime --bench uapi_dispatch
cargo bench -p layer36-runtime --bench startup
scripts/check-benchmark-regression.sh
```

The benchmark uses a no-op host adapter. That means it measures Layer36
dispatcher and policy overhead, not disk speed, terminal speed, or network
speed.

The current Phase 2 regression baseline is stored in:

- `docs/book/src/phase2/benchmark-baseline.json`

The checker supports:

- `BENCH_REGRESSION_MODE=warn` for warning-only reports
- `BENCH_REGRESSION_MODE=fail` for hard gate mode
- `BENCH_REGRESSION_THRESHOLD_PCT=<n>` for threshold tuning
- `BENCH_BASELINE_FILES=<path[:path...]>` for custom baseline sets

Current CI usage:

- Hosted full benchmark job uses `warn` mode against the Phase 2 baseline file.
- Self-hosted CI can run the same check in `fail` mode for strict local gating.

## Dispatch Baseline (2026-05-05)

| Path | Local result | Phase 2 target | Notes |
|---|---:|---:|---|
| Default stdout grant | ~192 ns | < 1 us | Default low-risk IO capability. |
| Filesystem open with read grant | ~1.16 us | track | Path grant check plus adapter call. |
| File handle read with grant re-check | ~1.20 us | track | Re-checks the opened file path before read. |
| File handle write with grant re-check | ~1.19 us | track | Re-checks the opened file path before write. |
| Missing filesystem read grant | ~579 ns | < 1 us | Denial path stops before adapter work. |
| HTTP fetch grant check | ~954 ns | < 1 us | URL endpoint parsing plus `net.connect` check. |

## What This Means

The first dispatcher path is fast enough for the Phase 2 target on the reference
machine. The harder work is still ahead: measuring full component startup,
cross-host variance, real adapter cost, and regressions over time.

For now, this gives us a useful line in the sand. UAPI checks are not free, but
they are small enough that safety is not fighting the basic CLI performance
goal.

## Runtime Startup Benchmarks

The startup benchmark now measures real Phase 2 components too. This is still an
in-process runtime benchmark, not a full shell command benchmark. It measures
Wasmtime setup, component loading, UAPI linking, policy checks, and adapter calls
without including terminal process startup time.

Build the fixtures first:

```bash
scripts/build-phase1-components.sh
scripts/build-phase2-smoke-component.sh
scripts/build-layer36-clock-component.sh
cargo bench -p layer36-runtime --bench startup
```

## Startup Baseline (2026-05-05)

| Path | Local result | Phase 2 target | Notes |
|---|---:|---:|---|
| Compile Phase 2 smoke component from bytes | ~2.86 ms | track | Wasmtime component compile path. |
| Cold runtime + run Phase 2 smoke app | ~3.16 ms | < 150 ms | Reads a granted file, uses time, locale, and stdout. |
| Run preloaded Phase 2 smoke app | ~81.05 us | track | UAPI calls with component already loaded. |
| Run preloaded `layer36-clock` with fixed time | ~49.92 us | track | Time, locale, and stdout path. |

The first cold runtime number is comfortably below the Phase 2 startup target on
the reference machine. We still need a full CLI benchmark with `hyperfine`,
cross-host numbers, and warning-only regression tracking before this exit gate
can be marked done.
