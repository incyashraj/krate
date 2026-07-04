# ADR-0001: Rust for the Krate runtime

**Status:** Accepted  
**Date:** 2026-05-01  
**Authors:** @incyashraj  
**Supersedes:** —  
**Superseded by:** —

---

## Context

The Krate runtime is a native binary that ships on every host operating
system we support (Windows, macOS, Linux, iOS, Android, browsers via
WASM). Its job is to load a WebAssembly component, enforce a capability
model, and dispatch UAPI calls to per-OS adapters.

The language we write it in is one of the most consequential early decisions:

- It will never be rewritten at scale. We choose once.
- It shapes the contributor pool for the life of the project.
- It determines which WebAssembly runtime we can embed naturally.
- It affects binary size, startup time, and memory footprint — all of which
  matter for Phase 4 (mobile) and Phase 7 (v1.0).

---

## Decision

We write the Krate runtime in **Rust**.

Specifically:

- `crates/runtime/` — the core runtime library.
- `crates/cli/` — the `krate` command-line binary.
- `crates/host-adapter/*` — per-OS adapter crates.
- `crates/bundle/`, `crates/policy/`, and future crates.

Apps themselves are compiled from any language to WebAssembly. Rust is the
runtime's implementation language, not a constraint on apps.

---

## Alternatives considered

### C++

**Rejected.**

Mature ecosystem, wide OS support. But: memory safety is manual. A runtime
with a single memory-safety CVE undermines the trust the entire platform
depends on. Build systems (CMake, Bazel, Meson) are a collective weakness;
contributor friction is higher than Cargo.

### Zig

**Rejected (for now).**

Compelling language: small, fast, simple. But: pre-1.0, stdlib churn is heavy,
ecosystem for WebAssembly runtimes is immature. Revisit only if Rust becomes
a limiting factor — no plausible path to that today.

### Go

**Rejected.**

Excellent tooling, great concurrency. But: GC introduces latency spikes
incompatible with 60fps UI on mobile (Phase 4). CGo overhead for embedding a
WASM engine in native UIs is material. Binary size is larger than Rust for
equivalent functionality.

### Kotlin / Swift

**Rejected.**

Platform-biased (JVM, Apple). A truly cross-platform runtime shouldn't itself
be bound to one ecosystem. Cross-compilation story for iOS and Android from
anything but the respective first-party toolchain is fragile.

---

## Consequences

### Positive

- Memory safety without GC — a rare combination that exactly fits runtime
  requirements.
- Wasmtime, our chosen WebAssembly engine (see ADR-0002), is itself Rust.
  Embedding is native.
- `cargo` gives us a reproducible build system from day one.
- Contributor pool intersects strongly with systems-minded developers who want
  to build this kind of project.

### Negative

- Rust learning curve deters contributors without prior experience. Mitigation:
  strong onboarding docs, generous code review, "good first issue" pipeline.
- Compile times are non-trivial at scale. Mitigation: workspace structure,
  `sccache`, incremental compilation, careful dependency hygiene (`deny.toml`).
- Unsafe Rust will be necessary at adapter boundaries (FFI to Cocoa, Win32,
  JNI, Objective-C). This is a known cost; scoped and audited.

### Neutral

- Rust's ecosystem churn (toolchain releases every 6 weeks) requires discipline.
  We pin MSRV and bump deliberately.
- Some platforms (iOS, Android) require additional toolchain setup. Addressed
  phase-by-phase.

---

## Revisiting

This decision is revisitable only if:

1. A fundamental blocker emerges (e.g., Rust toolchain cannot target a platform
   we need to support).
2. A newer language offers meaningful advantages AND has the ecosystem maturity
   to compete with Rust for a 10-year project.

Neither condition is foreseeable in 2026. Expected lifetime of this decision:
decade-scale.

---

## References

- [Rust language](https://www.rust-lang.org/)
- [Wasmtime (Rust-embedded WASM runtime)](https://wasmtime.dev/)
- [ADR template](template.md)
