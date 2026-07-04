# Phase 1 Benchmarks

These numbers are the first local baseline for the proof-of-concept runtime.
They are not product performance promises; they give us a repeatable way to see
whether future runtime changes move Krate in the right direction.

## Reference Machine

| Field | Value |
|---|---|
| Date | 2026-05-02 |
| CPU | Apple M4 |
| Cores | 10 physical / 10 logical |
| Memory | 16 GB |
| OS | macOS 26.3.1 (25D2128) |
| Architecture | arm64 |
| Rust | rustc 1.91.1 |
| Wasmtime | 43.0.2 |

## Microbenchmarks

Run with:

```bash
scripts/build-phase1-components.sh
cargo bench -p krate-runtime --bench startup
scripts/check-benchmark-regression.sh
```

| Metric | Baseline | Phase 1 target | Notes |
|---|---:|---:|---|
| Wasmtime engine construction | 864 ns | < 100 ms | Measures `Runtime::new`. |
| `Component::from_binary` for hello-world | 2.366 ms | < 20 ms | Measures component compilation from bytes. |
| Cold runtime run to `run()` completion | 2.451 ms | < 200 ms | Includes runtime creation, compile, instantiate, and one host print to a sink. |
| First host `print` on a loaded component | 10.067 us | Track | Measures instantiate plus one host call with output suppressed. |
| 1,000 host `print` calls on a loaded component | 88.189 us | < 1 us/call | Criterion throughput: about 88 ns per call including loop overhead. |
| RSS after hello-world exits | 14.9 MiB | < 40 MiB | Measured with `/usr/bin/time -l` on macOS. |

The committed JSON baseline lives at
`docs/book/src/phase1/benchmark-baseline.json`. CI runs the same benchmark suite
and emits a warning if a metric is more than 10% slower than the recorded
baseline. It does not fail the build during Phase 1 because GitHub-hosted
runners are noisy.

## Binary Size

| Artifact | Size |
|---|---:|
| `target/release/krate` | 11 MB |
| `dist/krate-0.1.0-dev-aarch64-apple-darwin.tar.gz` | 4.4 MB |

## Notes

- The host output is routed to an in-process sink for microbenchmarks so terminal
  I/O does not dominate host-call dispatch measurements.
- There is no AOT cache yet. Phase 2 can use this baseline to prove cache wins.
- The benchmark suite is small enough to run in CI, but the
  published baseline should be refreshed only from the documented reference
  machine.
