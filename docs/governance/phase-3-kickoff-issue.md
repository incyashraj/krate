# Phase 3 Kickoff Issue Draft

**Title:** `phase 3: build the first Layer36 desktop UI surface`

## Status

Draft only. Do not open this issue until Phase 2 exit review passes.

## Objective

Start Phase 3 after Phase 2 has a stable CLI UAPI and enough evidence to trust
it as the base for desktop UI work.

The Phase 3 sentence is:

> Run one Layer36 windowed app on Windows, macOS, and Linux.

## Why This Starts After Phase 2

Phase 3 depends on Phase 2. The UI layer will still need files, time, locale,
network, capability checks, adapters, samples, docs, and CI discipline. If those
are unstable, UI work will hide platform bugs instead of solving them.

## Prerequisites

- Phase 2 exit evidence is green for the final commit.
- UAPI v0.1 is frozen for `io`, `fs`, `net`, `time`, and `locale`.
- `layer36-clock`, `layer36-cat`, and `layer36-curl` have current evidence.
- Host adapter evidence exists for Linux, macOS, and Windows.
- UCap deny-before-adapter evidence is current.
- Rust SDK evidence is current.
- TypeScript evidence is current for the supported Phase 2 lane.
- Go runtime parity is either import-pure or explicitly experimental.
- The outside Rust walkthrough packet is filled and checked.
- Phase 2 retrospective is published.

## Initial Task Slice

- Write ADR-0013 for the Phase 3 widget strategy.
- Draft the `ui`, `gfx`, and `audio` WIT module boundaries.
- Add a Phase 3 UAPI reference scaffold.
- Add the first desktop window host-adapter spikes:
  - macOS AppKit window
  - Windows window shell
  - Linux GTK4 window
- Add a minimal `layer36-notes` sample plan.
- Define first-paint and frame-time measurements before feature work expands.
- Add an accessibility checklist before widget implementation begins.

## Non-Goals

- Mobile hosts.
- Browser host.
- App store packaging.
- Plugin systems.
- Full design system.
- Game engine scope.
- Signed bundles.

## Exit Signal

Phase 3 is ready to close when `layer36-notes` runs as a real windowed Layer36
app on Windows, macOS, and Linux, with input, layout, rendering, accessibility
checks, and frame-time evidence.

## References

- `Plan/Phase-3-Plan.md`
- `Plan/Build-Plan.md`
- `docs/book/src/phases/phase-3.md`
- `docs/book/src/phase2/exit-evidence.md`
- `docs/book/src/phase2/exit-bundle.md`
- `docs/book/src/phase2/retro.md`
