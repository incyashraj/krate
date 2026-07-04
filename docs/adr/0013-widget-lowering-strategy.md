# ADR-0013: Widget Lowering Strategy

**Status:** Proposed  
**Date:** 2026-05-21  
**Authors:** @incyashraj  
**Supersedes:** -  
**Superseded by:** -

---

## Context

Phase 3 is where Krate becomes visible. Phase 2 proved that a component can
call a stable host API for files, network, time, locale, and standard I/O.
Phase 3 has to prove the same idea for desktop apps.

The hard part is not creating a window. The hard part is making a button, text
field, list, menu, scroll area, and text input path feel correct on Windows,
macOS, and Linux. Users notice when these details drift. They notice keyboard
shortcuts, focus behavior, scroll feel, menu behavior, text selection, IME, and
screen reader output.

Krate needs one app model, but it must not force every host to look the same.
The app should describe intent. The host should decide how to make that intent
feel native where possible.

---

## Decision

We will lower the Krate widget tree into native host widgets when the host has
a semantic match, and use a custom drawn fallback when it does not. This is
Answer D from the Phase 3 plan: a native widget tree with custom drawing for the
long tail.

---

## Rules

The first rule is the native three of five rule. A widget can become a first
class protocol widget only if at least three of Windows, macOS, Linux, iOS, and
Android have a native control that can represent the same meaning.

The second rule is semantic matching. A native widget is allowed only when it
matches the app's intent, not just its pixels. For example, a button can lower
to an `NSButton`, a Win32 button, or a GTK button. A freeform chart should not
pretend to be a native button just because it has a clickable area.

The third rule is fallback honesty. When a widget is custom drawn, the runtime
must still provide layout, input, accessibility metadata, and permission checks.
Custom drawing is not an escape hatch from platform behavior.

The fourth rule is host preference. If the abstract Krate model and the host
native convention disagree, the adapter should prefer the host convention unless
it breaks app correctness or cross-host security.

---

## Alternatives considered

### Draw every widget ourselves

Rejected for the default path. It gives full control and easier visual
consistency, but it tends to feel foreign on desktop platforms. It also forces
Krate to rebuild accessibility, IME behavior, focus behavior, and host
conventions from scratch.

### Call only native widgets

Rejected as the only path. It gives strong native feel for common controls, but
the platform matrix gets too wide. Some app surfaces have no native equivalent
on one or more hosts. A native only model would either leak host differences
into app code or block common UI patterns.

### Embed a browser

Rejected for Phase 3. A browser is useful for many products, but it is not the
Krate desktop goal. It increases memory use, changes the app feel, and makes
Krate look like a web shell instead of a platform layer.

### Use one cross-platform window toolkit as the real platform

Rejected for now. A toolkit may still help behind a host adapter, but Krate
should own the app contract. The UAPI must stay stable even if one host adapter
changes its internal toolkit later.

---

## Consequences

### Positive

- Common controls can feel native on each host.
- App code still sees one widget protocol.
- Custom surfaces remain possible without making every app a custom renderer.
- Accessibility and input rules can be part of the shared protocol from the
  start.

### Negative

- Host adapters need two paths: native lowering and custom drawing.
- Tests must check behavior per host, not only pixels.
- The widget protocol has to stay small and disciplined.

### Neutral

- Some widgets will intentionally look different across hosts.
- Phase 3 needs a native behavior rubric, not only automated tests.

---

## Revisiting

Revisit this decision if one of these conditions appears:

1. native lowering causes app visible behavior to diverge in ways we cannot
   document or test
2. custom drawn fallbacks become the default for most widgets
3. accessibility or IME support becomes weaker than a drawn or browser based
   approach
4. a later mobile phase shows that the native three of five rule blocks the app
   model we need

---

## References

- `Plan/Phase-3-Plan.md`
- `docs/rfc/0003-widget-protocol.md`
- `docs/book/src/phase3/widget-protocol.md`
