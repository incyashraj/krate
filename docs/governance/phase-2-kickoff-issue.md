# Phase 2 Kickoff Issue Draft

**Title:** `phase 2: ship UAPI v0.1 for useful CLI apps`

## Objective

Start Phase 2 only after the Phase 1 exit checklist is green or explicitly
waived. The Phase 2 sentence is:

> Ship the first useful cross-platform CLI app through our runtime.

## Prerequisites

- All Phase 1 exit criteria met or explicitly waived in `Plan/Phase-1-Plan.md`.
- `krate run hello.wasm` verified on Linux, macOS, and Windows.
- Release artifacts cut at least once from a tag such as `v0.1.0-rc1`.
- Phase 1 benchmarks and Threat Model v0.1 published.
- ADR-0002 and ADR-0003 accepted and merged.
- `wasm32-wasip2` target installed and ready for Phase 2 work.

## Initial Task Slice

- Define the UAPI v0.1 WIT module boundaries:
  - `io`
  - `fs`
  - `net`
  - `time`
  - `locale`
- Replace the temporary `krate:phase1/host` interface with Phase 2 UAPI
  imports.
- Add host adapter structure for Linux, macOS, and Windows.
- Add the first soft UCap manifest checks at UAPI call sites.
- Build the first three sample CLI apps:
  - `krate-cat`
  - `krate-curl`
  - `krate-clock`
- Extend CI so each sample builds and runs on all three desktop hosts.

## Non-Goals

- GUI, widgets, graphics, and native windowing.
- Mobile host shells.
- Marketplace, signing, identity, or update infrastructure.
- Strong capability persistence. Phase 2 is soft enforcement only.

## Exit Signal

Phase 2 is ready to close when a developer can write a Rust, Go, or TypeScript
CLI component that reads files, makes HTTP requests, prints output, and uses
time/locale primitives through Krate on Linux, macOS, and Windows.

## References

- `Plan/Phase-2-Plan.md`
- `Plan/Build-Plan.md`
- `docs/book/src/phases/phase-2.md`
- `docs/book/src/phase1/benchmarks.md`
- `docs/book/src/phase1/threat-model.md`
