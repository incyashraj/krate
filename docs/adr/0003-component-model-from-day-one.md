# ADR-0003: Component Model From Day One

**Status:** Accepted  
**Date:** 2026-05-02  
**Authors:** @incyashraj, Codex  
**Supersedes:** —  
**Superseded by:** —

---

## Context

Krate's Universal API is planned as WIT interfaces implemented by host
adapters. That makes the WebAssembly Component Model the natural execution unit:
it gives structured imports and exports, language-neutral bindings, and a path
to composing app modules later.

Phase 1 could be simpler if it accepted core WebAssembly modules only, but doing
so would make the first loader incompatible with the Phase 2 UAPI direction. The
project would then need to rewrite the loader exactly when it should be proving
the first useful UAPI modules.

---

## Decision

We will require WebAssembly components, not raw core modules, for Krate
runtime inputs from Phase 1 onward.

The first runtime slice validates and instantiates components and calls a
zero-argument `run` export when present. The next Phase 1 slice adds the
temporary `krate:phase1/host` WIT interface for `print` and `exit`.

---

## Alternatives Considered

### Start With Core Modules

Rejected. This would make the first demo easier but would push a loader rewrite
into Phase 2, when the team needs to focus on UAPI design and host adapters.

### Support Both Core Modules And Components

Rejected for Phase 1. Dual support increases test matrix and error-handling
surface before the project has a single successful path.

### Delay WIT Until Phase 2

Rejected. WIT is central to Krate's UAPI story, so Phase 1 should force the
toolchain and runtime shape into the same direction early.

---

## Consequences

### Positive

- Loader architecture matches the UAPI plan.
- Early tests exercise the toolchain Krate actually wants developers to use.
- Future host imports map cleanly to WIT interfaces.

### Negative

- Hello-world setup is more complex than raw `.wasm`.
- Developers need `cargo-component` or equivalent tooling earlier.
- Component Model APIs are still evolving quickly.

### Neutral

- Phase 1 docs must explain why raw WebAssembly modules are intentionally not the
  target.

---

## Revisiting

Revisit only if Component Model tooling becomes a near-term blocker for the
desktop proof of concept. Even then, prefer a temporary fixture-generation
workaround over changing the runtime input model.

---

## References

- [WebAssembly Component Model](https://component-model.bytecodealliance.org/)
- [WIT](https://component-model.bytecodealliance.org/design/wit.html)
- [cargo-component](https://github.com/bytecodealliance/cargo-component)
