# ADR-0002: Wasmtime As Runtime Engine

**Status:** Accepted  
**Date:** 2026-05-02  
**Authors:** @incyashraj, Codex  
**Supersedes:** —  
**Superseded by:** —

---

## Context

Krate needs an embeddable WebAssembly runtime that can run the Component Model
from Phase 1 onward. The runtime must work across desktop hosts first, then leave
a credible path to mobile hosts, capability enforcement, fuel metering, and
resource limits.

The engine choice affects the core loader API, host-function registration,
debuggability, CI time, release artifact size, and the contributor learning
curve. Replacing the engine after Phase 1 would likely require rewriting the
runtime crate and most integration tests.

---

## Decision

We will use **Wasmtime** as the Krate runtime engine.

Phase 1 pins `wasmtime = 43.0.2` because the current latest release, `44.0.1`,
requires Rust `1.92.0`, while this repository is pinned to Rust `1.91.1`.

---

## Alternatives Considered

### Wasmer

Rejected for Phase 1. Wasmer is mature for many embedding cases, but Wasmtime has
the stronger Component Model story and closer alignment with the Bytecode
Alliance ecosystem Krate is building on.

### WasmEdge

Rejected for Phase 1. WasmEdge remains interesting for mobile and edge scenarios,
but adopting it first would increase uncertainty around the Component Model and
the Rust embedding surface.

### Custom Runtime

Rejected. A custom WebAssembly runtime would turn Krate into a VM project
before it becomes an application platform. The goal is the UAPI, capability
model, host adapters, and distribution layer, not reimplementing a mature WASM
engine.

---

## Consequences

### Positive

- Strong Component Model support from day one.
- Rust-native embedding API.
- Bytecode Alliance stewardship and ecosystem alignment.
- Built-in support for fuel metering and resource control.

### Negative

- Wasmtime is a large dependency and increases compile time.
- Version cadence is fast and tied to Rust MSRV changes.
- Mobile constraints, especially iOS JIT restrictions, still require later
  design work.

### Neutral

- We pin the version deliberately and revisit during phase boundaries rather than
  auto-upgrading.

---

## Revisiting

Revisit if Wasmtime blocks a required target host, if mobile embedding becomes
impractical, or if another engine materially surpasses Wasmtime in Component
Model support while preserving Rust embedding quality.

---

## References

- [Wasmtime](https://wasmtime.dev/)
- [Wasmtime crate](https://crates.io/crates/wasmtime)
- [Bytecode Alliance](https://bytecodealliance.org/)
