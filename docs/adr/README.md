# Architecture Decision Records

This directory contains all Architecture Decision Records for Krate.

## What is an ADR?

An ADR is a short document that captures an important architectural decision:
what was decided, why, and what the consequences are. It is written once,
merged into `main`, and then **never modified** — only superseded by a new ADR.

## Why write ADRs?

- Technical decisions made verbally vanish. ADRs make them findable.
- New contributors can see *why* the code is the way it is, not just *what* it does.
- They create accountability: a decision that is hard to write down is a decision
  that hasn't been thought through.

## Index

| ADR | Title | Status | Phase |
|-----|-------|--------|-------|
| [0001](0001-rust-for-runtime.md) | Rust for the Krate runtime | Accepted | 0 |
| [0002](0002-wasmtime-runtime-engine.md) | Wasmtime as runtime engine | Accepted | 1 |
| [0003](0003-component-model-from-day-one.md) | Component Model from day one | Accepted | 1 |
| [0006](0006-wit-versioning-strategy.md) | WIT versioning strategy | Accepted | 2 |
| [0007](0007-ucap-soft-enforcement.md) | UCap v0.1 soft enforcement | Accepted | 2 |
| [0008](0008-host-async-runtime.md) | Host async runtime | Accepted | 2 |
| [0009](0009-sandbox-link-semantics.md) | Sandbox link-semantics guardrails | Accepted | 2 |
| [0010](0010-locale-timezone-discovery-fallbacks.md) | Locale and timezone discovery fallbacks | Accepted | 2 |
| [0011](0011-phase2-benchmark-regression-policy.md) | Phase 2 benchmark regression policy | Accepted | 2 |
| [0012](0012-adapter-crate-split-per-os.md) | Adapter crate split per host OS | Accepted | 2 |
| [0013](0013-widget-lowering-strategy.md) | Widget lowering strategy | Proposed | 3 |
| [0014](0014-layout-engine-taffy.md) | Layout engine uses Taffy | Proposed | 3 |

## Process

See [the ADR process guide](../book/src/contributing/adrs.md) for the full
workflow, or the quick version:

1. Copy `template.md` → `NNNN-short-title.md`
2. Fill out every section
3. Open a PR titled `ADR: <title>`
4. Merge after 2 approvals (or 1 approval + 7 days)
5. Update the index above
