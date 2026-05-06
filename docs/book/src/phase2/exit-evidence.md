# Phase 2 Exit Evidence

This page tracks the evidence needed before we say Phase 2 is complete.

Phase 2 is not finished just because the code runs on one machine. It is
finished when the UAPI contract, host adapters, language paths, samples,
performance checks, docs, and CI evidence all line up.

## How To Read This Page

Status meanings:

- **Done** means the gate is complete for the current Phase 2 scope.
- **Strong draft** means the design and local checks are solid, but the final
  freeze or external proof is still pending.
- **Partial** means useful work exists, but at least one planned proof is still
  missing.
- **Pending** means the gate still needs its first real proof.
- **Blocked** means we know what is needed, but another decision or toolchain
  step must happen first.

## Exit Gate Ledger

| Gate | Criterion | Status | Evidence | Next step |
|---|---|---|---|---|
| P2E-01 | UAPI modules frozen | **Strong draft** | `scripts/check-uapi.sh`, [UAPI Freeze Evidence](uapi-freeze-evidence.md) | Run the final freeze review and record the decision. |
| P2E-02 | Desktop host adapters | **Partial** | `scripts/check-adapter-boundary.sh`, [Adapter Boundary](adapter-boundary.md) | Keep collecting Linux, macOS, and Windows full-gate evidence. |
| P2E-03 | Rust bindings usable | **Partial** | `scripts/smoke-rust-sdk.sh`, [First Rust CLI](../uapi/first-rust-cli.md) | Publish only after UAPI v0.1 is frozen. |
| P2E-04 | Go bindings usable | **Blocked** | `scripts/build-phase2-go-variant-smoke.sh`, `scripts/promote-phase2-go-runtime-fixtures.sh` | Make TinyGo outputs Layer36 import-pure or mark Go experimental for this exit. |
| P2E-05 | TypeScript bindings usable | **Partial** | `scripts/build-phase2-language-variant-fixtures.sh`, `scripts/test-phase2-language-variants.sh` | Keep TS curl success evidence stable across restricted runners. |
| P2E-06 | curl cross-host | **Partial** | `apps/layer36-curl`, `tests/cli.rs` | Record identical stdout on Linux, macOS, and Windows. |
| P2E-07 | cat cross-host | **Partial** | `apps/layer36-cat`, `tests/cli.rs` | Record identical stdout on Linux, macOS, and Windows. |
| P2E-08 | clock cross-host | **Partial** | `apps/layer36-clock`, `tests/cli.rs` | Record fixed-time output on Linux, macOS, and Windows. |
| P2E-09 | UCap enforcement | **Partial** | `crates/policy`, `crates/runtime`, `tests/cli.rs` | Finish cross-host denial evidence at each UAPI boundary. |
| P2E-10 | Startup performance | **Partial** | `cargo bench -p layer36-runtime --bench startup`, [Dispatch Benchmarks](benchmarks.md) | Add full CLI startup measurements and cross-host baselines. |
| P2E-11 | Dispatch performance | **Partial** | `cargo bench -p layer36-runtime --bench uapi_dispatch`, `scripts/check-benchmark-regression.sh` | Collect stable cross-host benchmark evidence. |
| P2E-12 | Timed developer walkthrough | **Pending** | [First Rust CLI](../uapi/first-rust-cli.md) | Ask an outside Rust developer to build a small app and time the run. |
| P2E-13 | Generated UAPI reference | **Done** | `scripts/generate-uapi-reference.sh`, [Generated Reference](../reference/uapi/index.md) | Keep CI freshness checks enabled. |
| P2E-14 | WIT style guide | **Done** | [WIT Style Guide](../wit-style.md) | Keep using it during freeze review. |
| P2E-15 | ADR set | **Done** | `docs/adr/0006-wit-versioning.md` through `docs/adr/0012-adapter-crate-split.md` | Add a freeze ADR only if the final review changes a rule. |

## What Is Already Strong

The current direction is still right.

Layer36 has the correct shape for a universal software layer: app code calls a
portable UAPI, the runtime checks policy, and host adapters translate approved
calls to the native operating system. That is the right path for the larger
6 by 6 goal because it avoids hardcoding one host model into every app.

The strongest Phase 2 pieces today are:

- the UAPI contract shape for `io`, `fs`, `net`, `time`, and `locale`
- capability parsing, launch grants, and runtime boundary checks
- Rust sample apps for clock, cat, and curl
- generated docs and CI freshness checks
- adapter split structure across Linux, macOS, and Windows crates

## What Still Blocks Phase 2 Exit

The remaining work is mostly proof:

1. freeze UAPI v0.1 after review
2. collect clean Linux, macOS, and Windows evidence for the same samples
3. decide the Go track honestly, based on import purity
4. run longer fuzz and benchmark evidence
5. do one timed external walkthrough
6. record the retrospective and Phase 3 kickoff

## Local Evidence Commands

Run these before a Phase 2 exit review:

```bash
scripts/check-uapi.sh
scripts/generate-uapi-freeze-evidence.sh
scripts/check-adapter-boundary.sh
scripts/check-phase2-exit-evidence.sh
scripts/smoke-rust-sdk.sh
scripts/build-phase2-language-variant-fixtures.sh
scripts/test-phase2-language-variants.sh
```

For performance and soak checks:

```bash
cargo bench -p layer36-runtime --bench startup
cargo bench -p layer36-runtime --bench uapi_dispatch
scripts/check-benchmark-regression.sh
scripts/run-phase2-fuzz-smoke.sh
```

## CI Evidence We Still Need

For formal exit, save the run links in the Phase 2 plan:

- one recent hosted CI green run
- one recent self-hosted full gate green run on macOS ARM64
- Linux and Windows hosted or trusted runner evidence for the sample outputs
- one longer self-hosted fuzz run after the final UAPI freeze candidate
- one benchmark baseline check after the final sample set is fixed

That is enough to move Phase 2 from strong engineering progress to a clean
phase exit.
