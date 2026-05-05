# ADR-0011: Phase 2 Benchmark Regression Policy

**Status:** Accepted  
**Date:** 2026-05-05  
**Authors:** @incyashraj  
**Supersedes:** -  
**Superseded by:** -

---

## Context

Phase 2 now has two runtime benchmark surfaces that matter for day to day
engineering:

- startup path benchmarks (`crates/runtime/benches/startup.rs`)
- UAPI dispatch microbenchmarks (`crates/runtime/benches/uapi_dispatch.rs`)

Until now, benchmark checks existed but were only loosely enforced. This created
two practical problems. First, hosted CI logs were noisy because old Phase 1
baselines did not reflect current runtime shape. Second, there was no reliable
strict gate for local full validation before shipping larger runtime changes.

We need one clear policy that keeps hosted CI affordable and informative, while
still giving maintainers a strict local gate to catch real regressions early.

---

## Decision

We will use a dedicated Phase 2 benchmark baseline file and a two-mode
regression policy: hosted full benchmark CI runs in warning mode, and
self-hosted full CI can run in fail mode against the same Phase 2 baseline.

The baseline source of truth is `docs/book/src/phase2/benchmark-baseline.json`,
and refresh is done by `scripts/record-phase2-benchmark-baseline.sh`.

---

## Alternatives considered

### Keep warning-only checks everywhere

Rejected. This is easy to run, but it does not protect us from quietly shipping
large regressions in core runtime paths.

### Make hosted CI fail on regressions by default

Rejected for this phase. Hosted minutes are constrained, and baseline drift
across shared runners can create noisy failures that slow development.

### Keep Phase 1 and Phase 2 metrics mixed in one strict gate

Rejected. For current Phase 2 goals, mixed baselines produce avoidable noise and
hide the signal we care about.

---

## Consequences

### Positive

- Phase 2 benchmark signal is clearer in hosted CI.
- Local self-hosted full gate can enforce strict performance checks.
- Baseline refresh is reproducible and script-based.

### Negative

- We now maintain one more governance artifact (Phase 2 baseline file).
- Strict checks depend on maintainers running self-hosted full CI regularly.

### Neutral

- Phase 1 historical baseline is still available for reference, but no longer
  drives Phase 2 hosted regression warnings by default.

---

## Revisiting

Revisit this decision when one of these is true:

1. hosted runner variance is low enough to make fail mode practical
2. cross-host benchmark evidence is complete for Phase 2 exit
3. Phase 3 introduces new benchmark classes that need different thresholds

---

## References

- `scripts/check-benchmark-regression.sh`
- `scripts/record-phase2-benchmark-baseline.sh`
- `.github/workflows/ci.yml`
- `.github/workflows/self-hosted-ci.yml`
