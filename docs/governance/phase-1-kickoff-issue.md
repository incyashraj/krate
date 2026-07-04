# Phase 1 Kickoff Issue Draft

**Title:** `phase 1: prove one WASM component runs on three desktop hosts`

## Objective

Start Phase 1 once the Phase 0 exit checklist is green or explicitly waived.
The Phase 1 sentence is:

> Prove one binary runs identically on three desktop hosts.

## Prerequisites

- Phase 0 local checks green.
- Branch protection enabled on `main`.
- Docs site published.
- Naming decision recorded or codename risk accepted.
- At least one external contributor PR merged, unless intentionally waived.

## Initial Task Slice

- Create `crates/runtime`.
- Create `crates/cli`.
- Add ADR-0002 for Wasmtime as runtime engine.
- Add ADR-0003 for Component Model from day one.
- Add the first hello-world WASM component fixture.
- Make `krate run hello.wasm` print identical output on Linux, macOS, and
  Windows.

## References

- `Plan/Phase-1-Plan.md`
- `Plan/Build-Plan.md`
- `docs/book/src/phases/phase-0-status.md`
