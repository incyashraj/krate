# ADR-0012: Adapter Crate Split Per Host OS

**Status:** Accepted  
**Date:** 2026-05-05  
**Authors:** @incyashraj  
**Supersedes:** -  
**Superseded by:** -

---

## Context

Phase 2 adapter work now covers real filesystem, network, time, locale, and I/O
behavior through `krate-adapter-common` plus runtime-local host wiring.
That got us moving quickly, but it also mixed host-specific concerns into one
large implementation surface.

As Linux, macOS, and Windows behavior gets deeper, a single cfg-heavy adapter
path becomes harder to review and harder to test with confidence. We need clear
ownership boundaries so each host family can evolve without risking unrelated
host regressions.

The Phase 2 plan already calls for dedicated adapter crates per host OS. This
ADR locks that structure now so remaining adapter work follows one consistent
shape.

---

## Decision

We will keep shared logic in `crates/adapter-common` and split host-specific
adapter implementations into dedicated crates: `crates/adapter-linux`,
`crates/adapter-macos`, and `crates/adapter-windows`. Runtime integration will
use those crates as target-specific dependencies instead of growing one
cfg-gated monolith.

---

## Alternatives considered

### Keep a single runtime-local adapter with more cfg branches

Rejected. It is fast short term, but host-specific behavior becomes difficult to
reason about as branch count grows, and test ownership gets blurry.

### Keep one adapter crate with internal per-OS modules

Rejected for this phase. It is better than runtime-local sprawl, but still
keeps all host logic coupled in one package and increases cross-host compile
surface for routine changes.

### Build dynamic external host plugins now

Rejected for Phase 2 scope. That introduces packaging and plugin lifecycle work
before we finish the core cross-host adapter behavior.

---

## Consequences

### Positive

- Each host adapter can be implemented and tested with clearer boundaries.
- CI signals become easier to interpret by host family.
- `adapter-common` stays focused on deterministic cross-host rules.

### Negative

- More crates and manifests to maintain.
- Cross-crate trait and API changes need tighter coordination.

### Neutral

- Current runtime-local adapter wiring can continue as a transition path while
  per-OS crates land incrementally.

---

## Revisiting

Revisit this decision if one of these conditions appears:

1. host behavior converges enough that per-OS crates create unnecessary churn
2. we adopt a plugin host architecture that replaces compile-time host crates
3. CI and ownership data show the split no longer improves reliability

---

## References

- `Plan/Phase-2-Plan.md` (`P2-ADPT-01`, `P2-ADPT-02`, `P2-ADPT-03`)
- `crates/adapter-common/`
- `crates/runtime/src/lib.rs`
