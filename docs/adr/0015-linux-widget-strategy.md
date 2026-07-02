# ADR-0015: Linux Widget Strategy For Phase 3

**Status:** Proposed
**Date:** 2026-07-02
**Authors:** @incyashraj
**Supersedes:** the Linux native-widget rows in `Plan/Phase-3-Plan.md` §7.3 and §15.1 as originally written
**Superseded by:** -

---

## Context

ADR-0013 chose native widget lowering with a custom drawn fallback. The Phase 3
plan then made two technology choices for Linux: `winit` for windowing and
`gtk4-rs` for native widgets, in the same windows.

Those two choices do not compose. GTK4 removed foreign-window embedding: a GTK4
widget can only live inside a GTK-owned window that runs GTK's own event loop.
There is no supported way to host a `GtkButton` inside a winit-created Wayland
or X11 surface. Continuing on the written plan would spend the Linux widget
budget discovering this the hard way.

The same problem does not exist on the other desktop hosts. On macOS the
adapter owns AppKit directly, so native lowering works. On Windows, Win32
common controls can be created as child HWNDs of a winit-owned window, so
native lowering works there too.

---

## Decision

For Phase 3 / UAPI v0.1:

1. Linux keeps `winit` for windowing.
2. All Linux widgets use the custom drawn fallback (vello), styled with a
   per-host theme token pack for visual fit.
3. Native GTK widget lowering is out of Phase 3 scope. It may return later as
   a separate GTK-owned-window backend if user demand justifies it.
4. Windows keeps native lowering via Win32 common controls as child windows.
   XAML Islands is out of v0.1 scope.
5. macOS keeps native AppKit lowering.

This narrows where ADR-0013's native path applies in v0.1. It does not reverse
ADR-0013: the widget protocol, the semantic-match rule, and the drawn-fallback
honesty rules are unchanged, and native lowering remains the goal wherever the
host backend can support it.

---

## Why drawn fallback on Linux, not GTK-owned windows

- Keeping winit preserves one shared windowing and event-loop path for Linux
  and Windows, which the adapter scaffolding already builds on.
- Linux is the host where "native look" is least defined. GNOME, KDE, and other
  desktops ship different toolkits and themes, so a well-drawn widget set costs
  the least user-facing credibility on Linux of any host.
- A GTK-owned-window backend would fork the window adapter model for one host
  and force GTK's event loop into the runtime boundary before the first
  cross-host proof exists.
- The drawn fallback must exist anyway (ADR-0013 rule three), so Linux v0.1
  exercises a path every host needs, rather than adding a third native bridge.

---

## Consequences

### Positive

- Removes an impossible integration before implementation spends weeks on it.
- Cuts the per-widget native matrix for v0.1 by one host.
- The drawn-fallback path gets a real host driving it from the start.

### Negative

- Linux apps will not use real GTK controls in v0.1, so exact GTK look and
  feel is not achieved on GNOME desktops.
- The drawn path must take accessibility, focus, and keyboard behavior
  seriously on Linux from day one; there is no native widget to inherit them
  from.

### Neutral

- The Phase 3 exit criterion "feels native on each host" is measured on Linux
  against a drawn-widget rubric: correct scroll physics, focus behavior,
  keyboard shortcuts, dark mode, and DPI handling — not GTK widget identity.

---

## Revisiting

Revisit this decision if:

1. Linux user feedback shows the drawn widget set is a real adoption blocker,
2. GTK grows a supported foreign-window embedding path,
3. a maintained alternative native toolkit binding proves embeddable under
   winit, or
4. a later phase adds a GTK-owned-window backend behind the same
   `WindowAdapter` trait without forking the app contract.

---

## References

- `docs/adr/0013-widget-lowering-strategy.md`
- `Plan/Phase-3-Plan.md` §7, §15
- `Plan/Plan-Amendments-2026-07.md` amendment A1
- `docs/book/src/phase3/widget-protocol.md`
