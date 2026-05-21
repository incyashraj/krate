# ADR process

Architecture Decision Records (ADRs) capture the significant technical
decisions made in Layer36: what was decided, why, and what the consequences are.

## When to write an ADR

Write an ADR when you make a decision that:

- Affects multiple crates, phases, or contributors
- Is difficult or costly to reverse
- Would otherwise be invisible in code

Examples that need an ADR:
- Choosing a new dependency for the runtime
- Changing the WIT versioning strategy
- Adding a new UAPI module
- Changing the bundle format

Examples that do **not** need an ADR:
- Bug fixes
- Test additions
- Documentation updates
- Refactors that don't change observable behavior

## Process

1. Copy `docs/adr/template.md` to `docs/adr/NNNN-short-title.md` (next sequential number).
2. Fill out every section. Be honest about alternatives rejected.
3. Open a PR titled `ADR: <title>`.
4. Minimum 2 maintainers approve, or 1 approve + 7 days open.
5. **Merged ADRs are immutable.** To supersede an ADR, write a new one that
   references the old one with `Supersedes: ADR-NNNN`.

## Index

| ADR | Title | Status |
|-----|-------|--------|
| [ADR-0001](../../../adr/0001-rust-for-runtime.md) | Rust for the Layer36 runtime | Accepted |
| [ADR-0002](../../../adr/0002-wasmtime-runtime-engine.md) | Wasmtime as runtime engine | Accepted |
| [ADR-0003](../../../adr/0003-component-model-from-day-one.md) | Component Model from day one | Accepted |
| [ADR-0006](../../../adr/0006-wit-versioning-strategy.md) | WIT versioning strategy | Accepted |
| [ADR-0007](../../../adr/0007-ucap-soft-enforcement.md) | UCap v0.1 soft enforcement | Accepted |
| [ADR-0008](../../../adr/0008-host-async-runtime.md) | Host async runtime | Accepted |
| [ADR-0009](../../../adr/0009-sandbox-link-semantics.md) | Sandbox link-semantics guardrails | Accepted |
| [ADR-0010](../../../adr/0010-locale-timezone-discovery-fallbacks.md) | Locale and timezone discovery fallbacks | Accepted |
| [ADR-0011](../../../adr/0011-phase2-benchmark-regression-policy.md) | Phase 2 benchmark regression policy | Accepted |
| [ADR-0012](../../../adr/0012-adapter-crate-split-per-os.md) | Adapter crate split per host OS | Accepted |
| [ADR-0013](../../../adr/0013-widget-lowering-strategy.md) | Widget lowering strategy | Proposed |
| [ADR-0014](../../../adr/0014-layout-engine-taffy.md) | Layout engine uses Taffy | Proposed |
