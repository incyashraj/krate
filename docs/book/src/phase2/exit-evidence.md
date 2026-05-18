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
| P2E-01 | UAPI modules frozen | **Strong draft** | `scripts/check-uapi.sh`, `scripts/check-uapi-freeze-lock.sh`, `scripts/check-phase2-freeze-decision.sh`, `scripts/record-phase2-uapi-freeze-review.sh`, [UAPI Freeze Evidence](uapi-freeze-evidence.md), [UAPI Freeze Lock](uapi-freeze-lock.md), [UAPI Freeze Review Evidence](uapi-freeze-review-evidence.md), [UAPI Freeze Decision Packet](uapi-freeze-decision.md) | Run the final freeze review and fill the decision packet after cross-host and language evidence are ready. |
| P2E-02 | Desktop host adapters | **Partial** | `scripts/check-adapter-boundary.sh`, `scripts/record-phase2-adapter-evidence.sh`, `scripts/compare-phase2-adapter-evidence.sh`, hosted full CI artifacts `adapter-evidence-<os>`, hosted full CI `Adapter evidence compare` job, [Adapter Boundary](adapter-boundary.md), [Adapter Evidence](adapter-evidence.md) | Keep collecting Linux macOS Windows evidence; adapter reports now include boundary, shared adapter behavior tests, and native adapter crate tests. |
| P2E-03 | Rust bindings usable | **Partial** | `scripts/smoke-rust-sdk.sh`, `scripts/record-phase2-rust-sdk-evidence.sh`, hosted CI artifact `rust-sdk-evidence`, [Rust SDK Evidence](rust-sdk-evidence.md), [First Rust CLI](../uapi/first-rust-cli.md) | Keep package and outside-workspace smoke evidence green; publish only after UAPI v0.1 is frozen. |
| P2E-04 | Go bindings usable | **Partial** | `scripts/build-phase2-go-variant-smoke.sh`, `scripts/promote-phase2-go-runtime-fixtures.sh`, `scripts/record-phase2-go-readiness-evidence.sh`, [Go Readiness Evidence](go-readiness-evidence.md), [Go Phase 2 Decision](go-phase2-decision.md) | Go is experimental for runtime parity in Phase 2; keep TinyGo smoke and import-purity checks, and revisit promotion when artifacts import only `layer36:*`. |
| P2E-05 | TypeScript bindings usable | **Partial** | `scripts/build-phase2-language-variant-fixtures.sh`, `scripts/test-phase2-language-variants.sh`, `scripts/record-phase2-language-variant-evidence.sh`, `scripts/compare-phase2-language-variant-evidence.sh`, hosted full CI artifacts `language-variant-evidence-<os>`, hosted full CI `language-variant-evidence-compare` job, [Language Variant Evidence](language-variant-evidence.md), `language_variants_curl_*` CLI tests | Keep TS curl success evidence stable across restricted runners and keep cross-language curl error parity green (`5`, `20`, `21` classes). |
| P2E-06 | curl cross-host | **Partial** | `scripts/record-phase2-sample-evidence.sh`, `scripts/compare-phase2-sample-evidence.sh`, hosted full CI artifacts `sample-evidence-<os>`, hosted full CI `Sample evidence compare` job, [Sample Evidence](sample-evidence.md) | Move curl from temporary blocked-host exception to strict all-host hash parity once hosted localhost fixture behavior is stable. The comparator still checks matching curl hashes across hosts that did run curl. |
| P2E-07 | cat cross-host | **Partial** | `scripts/record-phase2-sample-evidence.sh`, `scripts/compare-phase2-sample-evidence.sh`, hosted full CI artifacts `sample-evidence-<os>`, hosted full CI `Sample evidence compare` job, [Sample Evidence](sample-evidence.md) | Keep strict cross-host cat hash parity green in hosted and self-hosted evidence runs. |
| P2E-08 | clock cross-host | **Partial** | `scripts/record-phase2-sample-evidence.sh`, `scripts/compare-phase2-sample-evidence.sh`, hosted full CI artifacts `sample-evidence-<os>`, hosted full CI `Sample evidence compare` job, [Sample Evidence](sample-evidence.md) | Keep strict fixed-time clock hash parity green in hosted and self-hosted evidence runs. |
| P2E-09 | UCap enforcement | **Partial** | `crates/policy`, `crates/runtime`, `dispatcher_denies_all_non_default_boundaries_before_adapter`, `tests/cli.rs` (`language_variants_curl_permission_denied_matches_rust_go_ts`), `scripts/record-phase2-ucap-evidence.sh`, `scripts/compare-phase2-ucap-evidence.sh`, hosted full CI artifacts `ucap-enforcement-evidence-<os>`, hosted full CI `UCap enforcement evidence compare` job, [UCap Enforcement Evidence](ucap-enforcement-evidence.md) | Keep collecting clean Linux macOS Windows deny evidence for the full deny-before-adapter matrix. |
| P2E-10 | Startup performance | **Partial** | `cargo bench -p layer36-runtime --bench startup`, `record-phase2-cli-startup`, `scripts/record-phase2-benchmark-evidence.sh`, `scripts/compare-phase2-benchmark-evidence.sh`, [Dispatch Benchmarks](benchmarks.md), [Benchmark Evidence](benchmark-evidence.md) | Collect Linux macOS Windows benchmark reports for the same commit, then set a full-CLI startup threshold from that evidence. |
| P2E-11 | Dispatch performance | **Partial** | `cargo bench -p layer36-runtime --bench uapi_dispatch`, `scripts/check-benchmark-regression.sh`, `scripts/record-phase2-benchmark-evidence.sh`, `scripts/compare-phase2-benchmark-evidence.sh`, [Benchmark Evidence](benchmark-evidence.md), [Dependency Evidence](dependency-evidence.md) | Keep stable regression checks, cross-host benchmark evidence, and dependency-audit signoff linked to the final Phase 2 commit. |
| P2E-12 | Timed developer walkthrough | **Pending** | `scripts/record-phase2-walkthrough-template.sh`, `scripts/check-phase2-walkthrough-evidence.sh`, [First Rust CLI](../uapi/first-rust-cli.md), [Timed Walkthrough Evidence](walkthrough-evidence.md) | Ask an outside Rust developer to fill the walkthrough packet, time the run, and pass the filled-packet checker. |
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
3. keep the Go track experimental until import purity passes
4. run longer fuzz and benchmark evidence
5. do one timed external walkthrough
6. record the retrospective and Phase 3 kickoff

## Quick Readiness Command

To get the current gate count in one terminal command:

```bash
scripts/phase2-exit-readiness.sh
```

This does not replace the ledger. It reads this page and prints the current
number of done, strong-draft, partial, pending, and blocked gates. It also
prints the hard blockers so a returning session can start from the right place.

## Local Evidence Commands

Run these before a Phase 2 exit review:

```bash
scripts/record-phase2-exit-bundle.sh --strict
scripts/check-uapi.sh
scripts/generate-uapi-freeze-evidence.sh
scripts/generate-uapi-freeze-lock.sh
scripts/check-uapi-freeze-lock.sh
scripts/check-phase2-freeze-decision.sh
scripts/check-adapter-boundary.sh
scripts/record-phase2-adapter-evidence.sh
scripts/compare-phase2-adapter-evidence.sh
scripts/check-phase2-exit-evidence.sh
scripts/record-phase2-sample-evidence.sh
scripts/compare-phase2-sample-evidence.sh
scripts/record-phase2-rust-sdk-evidence.sh
scripts/record-phase2-language-variant-evidence.sh
scripts/compare-phase2-language-variant-evidence.sh
scripts/record-phase2-go-readiness-evidence.sh
scripts/record-phase2-ucap-evidence.sh
scripts/compare-phase2-ucap-evidence.sh
scripts/record-phase2-benchmark-evidence.sh
scripts/compare-phase2-benchmark-evidence.sh
scripts/record-phase2-dependency-evidence.sh
scripts/record-phase2-walkthrough-template.sh
scripts/check-phase2-walkthrough-evidence.sh <filled-walkthrough.md>
scripts/record-phase2-ci-stability-evidence.sh
scripts/record-phase2-self-hosted-evidence.sh
scripts/check-phase2-closeout-docs.sh
scripts/smoke-rust-sdk.sh
scripts/build-phase2-language-variant-fixtures.sh
scripts/test-phase2-language-variants.sh
scripts/phase2-exit-readiness.sh
```

The [Exit Bundle](exit-bundle.md) command gives the quick local review report.
The other commands are still useful when you need the detailed per-area reports.
The exit bundle now includes the closeout docs check and can optionally include
hosted CI stability and self-hosted full-gate evidence, so the final review
packet can carry current GitHub run history beside the local proof.

For performance and soak checks:

```bash
cargo bench -p layer36-runtime --bench startup
cargo bench -p layer36-runtime --bench uapi_dispatch
scripts/check-benchmark-regression.sh
scripts/check-dependencies.sh
scripts/run-phase2-fuzz-smoke.sh
```

## CI Evidence We Still Need

For formal exit, save the run links in the Phase 2 plan:

- one recent hosted CI green run
- one hosted CI stability report from `scripts/record-phase2-ci-stability-evidence.sh`
- one recent self-hosted full gate green run on macOS ARM64
- Linux and Windows hosted or trusted runner evidence for the sample outputs
- one longer self-hosted fuzz run after the final UAPI freeze candidate
- one benchmark baseline check after the final sample set is fixed
- one dependency evidence report for the final commit, with hosted advisory
  proof if local advisory DB locking is unavailable
- one final pass of `scripts/check-phase2-closeout-docs.sh` after the
  retrospective and Phase 3 kickoff issue are updated from draft to final

That is enough to move Phase 2 from strong engineering progress to a clean
phase exit.

The hosted CI stability recorder is documented in
[CI Stability Evidence](ci-stability-evidence.md).
