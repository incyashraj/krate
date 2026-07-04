# ADR-0014: Layout Engine Uses Taffy

**Status:** Proposed  
**Date:** 2026-05-21  
**Authors:** @incyashraj  
**Supersedes:** -  
**Superseded by:** -

---

## Context

Phase 3 needs one layout result that both native widgets and drawn fallback
surfaces can trust. A button lowered to AppKit, GTK, or Win32 needs the same
logical rectangle as a custom drawn canvas. Without a shared layout engine, each
host adapter would be tempted to lay out controls its own way, and the platform
would drift quickly.

Krate also needs a layout model developers already understand. Flex style
layout is the smallest useful common ground for stacks, lists, sidebars, note
editors, and basic tool surfaces. CSS Grid is useful later, but the first notes
app can start with flex.

The engine has to be embeddable in Rust, deterministic enough for tests, and
fast enough that a large widget tree can be recomputed within the Phase 3 frame
budget.

---

## Decision

We will use Taffy as the Phase 3 layout engine. Krate will keep a small
wrapper crate, `krate-layout`, between the runtime and Taffy so WIT-facing
types, widget IDs, error mapping, and future compatibility rules stay under our
control.

---

## Alternatives considered

### Write a custom flexbox engine

Rejected. It would give us full control, but flexbox has enough edge cases that
we would spend Phase 3 rebuilding a known hard thing instead of proving the
platform UI path.

### Let every host toolkit do layout

Rejected. That gives each platform too much freedom. Native controls can still
render natively, but the Krate runtime must own the portable layout result so
drawn fallback widgets, hit testing, snapshots, and accessibility bounds stay
consistent.

### Use a browser engine for layout

Rejected for Phase 3. It brings a much larger surface than we need and pushes
Krate toward a browser-shell architecture.

### Use a tiny stack-only layout permanently

Rejected. A stack-only layout is useful as a first smoke path, but `krate-notes`
needs scrolling, editor panes, lists, toolbars, and resizable regions. We need a
real layout engine before native widgets become deep.

---

## Consequences

### Positive

- The runtime gets one host-neutral layout result keyed by stable widget IDs.
- Native widgets and drawn fallback widgets can share hit-test and accessibility
  bounds.
- Taffy gives us flexbox behavior without writing a layout engine from scratch.
- The wrapper crate lets us swap or pin engine behavior later without exposing
  Taffy directly as the Krate API.

### Negative

- Taffy becomes part of the runtime dependency graph.
- Layout behavior may change when Taffy changes, so we need pinned versions and
  tests around Krate expectations.
- The first wrapper only covers the small style subset in the Phase 3 draft.

### Neutral

- Grid and advanced style support can land later through the same wrapper.
- The first layout proof is headless. It computes rectangles but does not draw
  or create native controls yet.

---

## Revisiting

Revisit this decision if one of these conditions appears:

1. Taffy cannot meet the 10,000-node layout budget after realistic widget
   styles are added
2. Taffy behavior changes in ways that break Krate's compatibility promises
3. native host layout proves necessary for a class of controls we cannot model
   at the runtime layer
4. accessibility or IME bounds require layout data Taffy cannot provide

---

## References

- `Plan/Phase-3-Plan.md`
- `crates/layout/`
- `docs/book/src/phase3/layout.md`
- <https://github.com/DioxusLabs/taffy>
