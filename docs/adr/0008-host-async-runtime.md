# ADR-0008: Host Async Runtime

**Status:** Accepted  
**Date:** 2026-05-04  
**Authors:** @incyashraj  
**Supersedes:** -  
**Superseded by:** -

---

## Context

Krate UAPI calls look synchronous to app code in Phase 2. A CLI app calls
`fs.read`, `net.fetch`, or `time.sleep` and waits for a result. The host side,
however, will eventually need async I/O for networking, timers, and platform
adapters.

The runtime must pick a host async strategy before network and adapter work
grows. The choice affects dependency shape, test behavior, startup cost, and how
we later move from a simple CLI runtime to desktop and mobile hosts.

---

## Decision

We will use Tokio as the host-side async runtime when Phase 2 adapters need
async work, while keeping the public Phase 2 UAPI synchronous from the app's
point of view.

The first runtime slices may stay blocking where that keeps the implementation
smaller and tests clearer. As soon as an adapter needs async DNS, HTTP, timers,
or platform event integration, it should use a small Tokio runtime owned by the
Krate host path, not by guest apps.

---

## Alternatives considered

### Fully Blocking Host Adapters

Rejected as the long-term direction. Blocking code is fine for the first local
adapter slices, but it becomes a poor fit for real HTTP, timers, and future GUI
or mobile hosts.

### async-std

Rejected. It is smaller in some cases, but Tokio has broader ecosystem support,
especially for HTTP, timers, and production Rust networking.

### Expose Async Directly In UAPI v0.1

Rejected for Phase 2. Async component-model support is still maturing, and
forcing early app authors to reason about async across the ABI would slow the
first useful CLI slice.

### One Global Multi-Threaded Runtime

Rejected for now. A global runtime can make tests and embedding harder. Phase 2
should keep runtime ownership explicit and small.

---

## Consequences

### Positive

- The host can use mature Rust networking and timer libraries.
- UAPI remains simple for first app authors.
- The runtime can grow toward real adapters without changing the app-facing WIT
  shape.

### Negative

- Tokio adds dependency weight and compile time.
- Careless blocking inside async paths can cause performance issues.
- Runtime ownership must be handled carefully when Krate is embedded inside
  GUI or mobile apps.

### Neutral

- Phase 2 can mix blocking local adapter code and Tokio-backed adapters while
  the public UAPI stays synchronous.

---

## Revisiting

Revisit this when Component Model async support is stable enough for Krate app
authors, or when embedding in a GUI or mobile host reveals runtime ownership
problems. Any change must keep existing Phase 2 apps runnable.

---

## References

- [Tokio](https://tokio.rs/)
- [WebAssembly Component Model async proposal](https://github.com/WebAssembly/component-model)
- [Wasmtime async support](https://docs.wasmtime.dev/api/wasmtime/struct.Config.html#method.async_support)
